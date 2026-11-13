//! GitHub organizations: adding and signing in, the cached API clients, and runner and job polls.

use std::sync::Arc;

use chrono::Utc;
use futures::future::join_all;

use super::{Engine, EngineError, SecretStore};
use crate::desired::model::Inventory;
use crate::gh;
use crate::github::{GithubClient, GithubError};
use crate::model::{
    AccountKind, AppConfig, GhRunner, OrgAuth, OrgConfig, OrgObservation, RepoScope, Timed, default_api_base,
};
use crate::sync::{MutexExt, RwLockExt};

#[derive(Debug, thiserror::Error)]
enum GithubAuthError {
    #[error("No token in the Keychain for this organization.")]
    MissingToken,
    #[error("Couldn't read the GitHub token from the Keychain: {0}")]
    Secret(String),
    #[error("{0}")]
    Gh(#[from] gh::GhError),
}

/// Where one kind of GitHub data, and the latest error fetching it, live in an org's observations.
type ObservationSlot<T> = fn(&mut OrgObservation) -> (&mut Option<Timed<T>>, &mut Option<Timed<String>>);

impl Engine {
    /// Keychain calls can block, so they run off the async runtime.
    async fn secrets_call<T: Send + 'static>(
        &self,
        call: impl FnOnce(&dyn SecretStore) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let secrets = self.secrets.clone();
        tokio::task::spawn_blocking(move || call(secrets.as_ref()))
            .await
            .map_err(|e| format!("The Keychain request stopped unexpectedly: {e}"))?
    }

    fn find_org(&self, login: &str) -> Result<OrgConfig, EngineError> {
        let key = login.to_lowercase();
        self.config()
            .orgs
            .into_iter()
            .find(|o| o.login.to_lowercase() == key)
            .ok_or_else(|| {
                EngineError::NotFound(format!(
                    "{login} isn't an added organization. Add it in Settings ▸ GitHub first."
                ))
            })
    }

    /// API client for an org, with its token from the Keychain or from `gh`.
    /// `Err` carries a user-facing reason (no token, gh missing or logged out).
    async fn github_client(&self, org: &OrgConfig) -> Result<Arc<GithubClient>, GithubAuthError> {
        let key = org.login.to_lowercase();
        if let Some(c) = self.github.locked().get(&key) {
            return Ok(c.clone());
        }
        let token = match org.auth {
            OrgAuth::Token => {
                let account = key.clone();
                self.secrets_call(move |secrets| secrets.get(&account))
                    .await
                    .map_err(GithubAuthError::Secret)?
                    .ok_or(GithubAuthError::MissingToken)?
            }
            OrgAuth::GhCli => gh::token(&gh::host_for_api_base(&org.api_base)).await?,
        };
        let client = Arc::new(GithubClient::new(&org.api_base, &token));
        self.github.locked().insert(key, client.clone());
        Ok(client)
    }

    /// GitHub errors, reworded for how this org signs in.
    fn github_error_text(&self, org: &OrgConfig, e: &GithubError) -> String {
        match (org.auth, e) {
            // A personal account's error already names the repository permission it needs.
            (_, GithubError::Forbidden(_)) if org.kind == AccountKind::User => e.to_string(),
            (OrgAuth::GhCli, GithubError::Forbidden(_)) => format!(
                "Your gh login can't list {}'s runners. GitHub requires the {} scope (and org owner rights) for that. Run `{}` in Terminal.",
                org.login,
                gh::RUNNERS_SCOPE,
                gh::refresh_command(&gh::host_for_api_base(&org.api_base))
            ),
            (OrgAuth::GhCli, GithubError::Unauthorized) => {
                "gh's token was rejected. Run `gh auth login` in Terminal; the app picks up the new login automatically.".into()
            }
            _ => e.to_string(),
        }
    }

    /// Forget a cached client so the next poll fetches a fresh token (e.g. after `gh auth refresh`).
    fn drop_github_client(&self, org: &OrgConfig) {
        self.github.locked().remove(&org.login.to_lowercase());
    }

    /// The orgs to poll. An account with repositories listed in the inventory
    /// reads exactly those, so its repositories are never listed.
    async fn polled_orgs(&self) -> Vec<PolledOrg> {
        let orgs = self.config().orgs;
        if orgs.is_empty() {
            return Vec::new();
        }
        // No inventory, or one that doesn't parse, leaves each org's own scope.
        let inventory = self.read_inventory().await.ok();
        orgs.into_iter()
            .map(|config| {
                let listed = listed_repos(inventory.as_ref(), &config.login);
                PolledOrg::new(config, listed)
            })
            .collect()
    }

    /// The account's repositories listed in the inventory, if it lists any.
    async fn inventory_repos_of(&self, login: &str) -> Option<Vec<String>> {
        listed_repos(self.read_inventory().await.ok().as_ref(), login)
    }

    /// Checks a credential the way polls will use it. With repositories listed in the
    /// inventory, it reads exactly their runners and never calls an `/orgs` endpoint
    /// or lists the account's repositories.
    async fn check_runners(&self, client: &GithubClient, org: &OrgConfig) -> Result<Vec<GhRunner>, GithubError> {
        match self.inventory_repos_of(&org.login).await {
            Some(repos) => client.listed_repo_runners(&org.login, &repos).await,
            None => client.runners(&org.login, org.kind, &org.repo_scope).await,
        }
    }

    /// Fetch one kind of GitHub data for every org, keeping the last good value beside the newest error.
    async fn poll_github<T>(
        &self,
        fetch: impl AsyncFn(&GithubClient, &PolledOrg) -> Result<T, GithubError>,
        refetch_token_on: fn(&GithubError) -> bool,
        slot: ObservationSlot<T>,
    ) {
        let orgs = self.polled_orgs().await;
        join_all(orgs.iter().map(|polled| async {
            let org = &polled.config;
            let key = org.login.to_lowercase();
            let client = match self.github_client(org).await {
                Ok(client) => client,
                Err(reason) => {
                    let mut obs = self.obs.write_locked();
                    let entry = obs.github.entry(key).or_default();
                    entry.missing_token = matches!(reason, GithubAuthError::MissingToken);
                    *slot(entry).1 = Some(Timed::new(Utc::now(), reason.to_string()));
                    return;
                }
            };
            let result = fetch(&client, polled).await;
            if result.as_ref().is_err_and(refetch_token_on) {
                self.drop_github_client(org);
            }
            let mut obs = self.obs.write_locked();
            let entry = obs.github.entry(key).or_default();
            entry.missing_token = false;
            entry.rate_limit = client.rate_limit();
            let (value, error) = slot(entry);
            match result {
                Ok(fetched) => {
                    *value = Some(Timed::new(Utc::now(), fetched));
                    *error = None;
                }
                Err(e) => *error = Some(Timed::new(Utc::now(), self.github_error_text(org, &e))),
            }
        }))
        .await;
    }

    pub async fn poll_github_runners(&self) {
        self.poll_github(
            async |client, polled| {
                let org = &polled.config;
                match &polled.listed {
                    // The inventory names the repositories, so their runners are read
                    // one repository at a time, for an organization too.
                    Some(repos) => client.listed_repo_runners(&org.login, repos).await,
                    None => client.runners(&org.login, org.kind, &org.repo_scope).await,
                }
            },
            // A 403 here means the credential can't list runners. Fetching the token
            // again picks up a fix such as `gh auth refresh --scopes admin:org`.
            |e| matches!(e, GithubError::Unauthorized | GithubError::Forbidden(_)),
            |org| (&mut org.runners, &mut org.error),
        )
        .await;
    }

    pub async fn poll_github_jobs(&self) {
        self.poll_github(
            async |client, polled| {
                let org = &polled.config;
                client.scan_account_jobs(&org.login, org.kind, &org.repo_scope).await
            },
            // Repositories the token can't read only mark the scan incomplete, so a 403
            // says nothing about the credential. Keeping the client keeps its ETag cache.
            |e| matches!(e, GithubError::Unauthorized),
            |org| (&mut org.jobs, &mut org.jobs_error),
        )
        .await;
    }

    /// Adds an org or a personal account after checking the credential can list its runners.
    /// `GhCli` uses the GitHub CLI's login; `Token` stores `token` in the Keychain.
    /// A personal account is detected from GitHub and read per repository.
    pub async fn add_org(
        &self,
        login: &str,
        auth: OrgAuth,
        token: Option<&str>,
        repo_scope: RepoScope,
        api_base: Option<String>,
    ) -> Result<(), EngineError> {
        let login = login.trim().trim_start_matches('@').to_string();
        if login.is_empty() || login.contains('/') || login.contains(' ') {
            return Err(EngineError::Invalid("Enter an organization login, like `acme`.".into()));
        }
        let key = login.to_lowercase();
        if self.config().orgs.iter().any(|o| o.login.to_lowercase() == key) {
            return Err(EngineError::Invalid(format!("{login} is already added.")));
        }
        let api_base = api_base.unwrap_or_else(default_api_base);
        let mut org = OrgConfig {
            login: login.clone(),
            api_base: api_base.clone(),
            repo_scope,
            added_at: Utc::now(),
            auth,
            kind: AccountKind::Organization,
        };
        let token = match auth {
            OrgAuth::GhCli => gh::token(&gh::host_for_api_base(&api_base))
                .await
                .map_err(|e| EngineError::Invalid(e.to_string()))?,
            OrgAuth::Token => {
                let t = token.map(str::trim).unwrap_or_default();
                if t.is_empty() {
                    return Err(EngineError::Invalid("A token is required.".into()));
                }
                t.to_string()
            }
        };
        let client = GithubClient::new(&api_base, &token);
        org.kind = client.account_kind(&login).await;
        let runners = self
            .check_runners(&client, &org)
            .await
            .map_err(|e| EngineError::Invalid(self.github_error_text(&org, &e)))?;

        if auth == OrgAuth::Token {
            let account = key.clone();
            self.secrets_call(move |secrets| secrets.set(&account, &token))
                .await
                .map_err(EngineError::Storage)?;
        }
        self.github.locked().insert(key.clone(), Arc::new(client));
        self.update_config(|c| c.orgs.push(org))?;
        let mut obs = self.obs.write_locked();
        let entry = obs.github.entry(key).or_default();
        entry.runners = Some(Timed::new(Utc::now(), runners));
        entry.missing_token = false;
        entry.error = None;
        Ok(())
    }

    /// Switches an org to the GitHub CLI login (after checking it works) and
    /// removes any stored token.
    pub async fn use_gh_for_org(&self, login: &str) -> Result<(), EngineError> {
        let key = login.to_lowercase();
        let mut org = self.find_org(login)?;
        org.auth = OrgAuth::GhCli;
        let token = gh::token(&gh::host_for_api_base(&org.api_base))
            .await
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        let client = GithubClient::new(&org.api_base, &token);
        self.check_runners(&client, &org)
            .await
            .map_err(|e| EngineError::Invalid(self.github_error_text(&org, &e)))?;
        self.update_config(|c| {
            if let Some(o) = c.orgs.iter_mut().find(|o| o.login.to_lowercase() == key) {
                o.auth = OrgAuth::GhCli;
            }
        })?;
        self.delete_token(&key).await;
        self.github.locked().insert(key, Arc::new(client));
        Ok(())
    }

    /// Re-reads gh's login on the next poll (after `gh auth refresh` / `login`).
    pub fn reload_gh_credentials(&self) {
        let orgs = self.config().orgs;
        let mut cache = self.github.locked();
        for o in orgs.iter().filter(|o| o.auth == OrgAuth::GhCli) {
            cache.remove(&o.login.to_lowercase());
        }
    }

    pub async fn update_org_token(&self, login: &str, token: &str) -> Result<(), EngineError> {
        let key = login.to_lowercase();
        let org = self.find_org(login)?;
        let token = token.trim().to_string();
        let client = GithubClient::new(&org.api_base, &token);
        self.check_runners(&client, &org).await?;
        let account = key.clone();
        self.secrets_call(move |secrets| secrets.set(&account, &token))
            .await
            .map_err(EngineError::Storage)?;
        self.update_config(|c| {
            if let Some(o) = c.orgs.iter_mut().find(|o| o.login.to_lowercase() == key) {
                o.auth = OrgAuth::Token;
            }
        })?;
        self.github.locked().insert(key, Arc::new(client));
        Ok(())
    }

    pub fn set_org_scope(&self, login: &str, scope: RepoScope) -> Result<(), EngineError> {
        let key = login.to_lowercase();
        self.update_config(|c| {
            if let Some(o) = c.orgs.iter_mut().find(|o| o.login.to_lowercase() == key) {
                o.repo_scope = scope;
            }
        })
    }

    pub async fn remove_org(&self, login: &str) -> Result<(), EngineError> {
        let key = login.to_lowercase();
        self.update_config(|c| c.orgs.retain(|o| o.login.to_lowercase() != key))?;
        self.delete_token(&key).await;
        self.github.locked().remove(&key);
        self.obs.write_locked().github.remove(&key);
        Ok(())
    }

    /// The org no longer uses a stored token, so a failed delete only leaves an unused Keychain item.
    async fn delete_token(&self, key: &str) {
        let account = key.to_string();
        if let Err(e) = self.secrets_call(move |secrets| secrets.delete(&account)).await {
            tracing::warn!("couldn't delete the stored GitHub token for {key}: {e}");
        }
    }
}

/// An added account as one poll sees it.
struct PolledOrg {
    /// The saved config, its scope replaced by the inventory's repositories when it lists some.
    config: OrgConfig,
    /// The account's repositories listed in the inventory, if any.
    listed: Option<Vec<String>>,
}

impl PolledOrg {
    /// Listed repositories replace the saved scope.
    fn new(config: OrgConfig, listed: Option<Vec<String>>) -> Self {
        match listed {
            Some(repos) => PolledOrg {
                config: OrgConfig {
                    repo_scope: RepoScope::Selected { repos: repos.clone() },
                    ..config
                },
                listed: Some(repos),
            },
            None => PolledOrg { config, listed: None },
        }
    }
}

/// The config as polls and views see it: each account with repositories listed
/// in the inventory has them as its scope. The saved config keeps its own scope.
pub(super) fn with_inventory_scopes(config: &AppConfig, inventory: Option<&Inventory>) -> AppConfig {
    AppConfig {
        orgs: config
            .orgs
            .iter()
            .cloned()
            .map(|org| {
                let listed = listed_repos(inventory, &org.login);
                PolledOrg::new(org, listed).config
            })
            .collect(),
        ..config.clone()
    }
}

/// `login`'s repositories in the inventory, or `None` when there is no inventory or it lists none.
fn listed_repos(inventory: Option<&Inventory>, login: &str) -> Option<Vec<String>> {
    inventory
        .map(|i| i.repositories_of(login))
        .filter(|repos| !repos.is_empty())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::Path;
    use axum::http::StatusCode;
    use axum::routing::get;
    use serde_json::json;

    use super::*;
    use crate::engine::MemorySecrets;

    /// A personal account whose `/orgs` and repository-list endpoints refuse with 403.
    /// Every request path is recorded.
    async fn personal_account(seen: Arc<Mutex<Vec<String>>>) -> String {
        let forbidden = || async {
            (
                StatusCode::FORBIDDEN,
                axum::Json(json!({"message": "Must have admin rights to Repository."})),
            )
        };
        let log = |seen: Arc<Mutex<Vec<String>>>| {
            move |req: axum::extract::Request, next: axum::middleware::Next| {
                let seen = seen.clone();
                async move {
                    seen.lock().unwrap().push(req.uri().path().to_string());
                    next.run(req).await
                }
            }
        };
        let app = Router::new()
            .route(
                "/users/{login}",
                get(|Path(login): Path<String>| async move { axum::Json(json!({"login": login, "type": "User"})) }),
            )
            .route("/orgs/{org}/actions/runners", get(forbidden))
            .route("/orgs/{org}/repos", get(forbidden))
            .route("/user/repos", get(forbidden))
            .route(
                "/repos/{owner}/{repo}/actions/runners",
                get(|Path((_owner, repo)): Path<(String, String)>| async move {
                    axum::Json(json!({"total_count": 1, "runners": [{
                        "id": if repo == "web" { 1 } else { 2 },
                        "name": format!("{repo}-runner"),
                        "os": "macOS",
                        "status": "online",
                        "busy": false,
                        "labels": [{"name": "self-hosted"}],
                    }]}))
                }),
            )
            .layer(axum::middleware::from_fn(log(seen)));
        crate::test_server::serve(app).await
    }

    fn engine_with_listed_repos(dir: &tempfile::TempDir) -> Engine {
        let inventory = dir.path().join("fleet.toml");
        let example = include_str!("../../../inventory/fleet.example.toml");
        std::fs::write(
            &inventory,
            format!("repositories = [\"solo/web\", \"solo/api\", \"elsewhere/tool\"]\n{example}"),
        )
        .unwrap();
        let engine = Engine::load(dir.path().join("config.json"), Arc::new(MemorySecrets::default())).unwrap();
        engine
            .set_inventory_path(Some(inventory.display().to_string()))
            .unwrap();
        engine
    }

    fn assert_only_listed_repos_read(seen: &Mutex<Vec<String>>) {
        let mut seen = seen.lock().unwrap().clone();
        assert!(
            !seen.iter().any(|p| p.starts_with("/orgs/") || p == "/user/repos"),
            "an explicit repository list makes no org call or account scan: {seen:?}"
        );
        seen.retain(|p| p.ends_with("/actions/runners"));
        seen.sort();
        assert_eq!(
            seen,
            vec!["/repos/solo/api/actions/runners", "/repos/solo/web/actions/runners"]
        );
    }

    #[tokio::test]
    async fn setup_with_listed_repositories_reads_only_their_runners() {
        let dir = tempfile::tempdir().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = personal_account(seen.clone()).await;
        let engine = engine_with_listed_repos(&dir);

        engine
            .add_org("solo", OrgAuth::Token, Some("fake"), RepoScope::None, Some(base))
            .await
            .unwrap();
        assert_eq!(engine.config().orgs[0].kind, AccountKind::User);
        let runners = engine.obs.read_locked().github["solo"]
            .runners
            .as_ref()
            .map(|r| r.value.len());
        assert_eq!(runners, Some(2));
        assert_only_listed_repos_read(&seen);
    }

    #[tokio::test]
    async fn token_update_with_listed_repositories_reads_only_their_runners() {
        let dir = tempfile::tempdir().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = personal_account(seen.clone()).await;
        let engine = engine_with_listed_repos(&dir);
        engine
            .add_org("solo", OrgAuth::Token, Some("fake"), RepoScope::None, Some(base))
            .await
            .unwrap();
        seen.lock().unwrap().clear();

        engine.update_org_token("solo", "replacement").await.unwrap();
        assert_only_listed_repos_read(&seen);
    }
}
