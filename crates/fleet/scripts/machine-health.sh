# Read-only health check. Missing metrics are empty, never synthesized as zero.
export LC_ALL=C
metric() { printf 'RFM\t%s\t%s\n' "$1" "$2"; }
case "$(uname -s)" in
  Darwin)
    metric os macos
    metric boot_id "$(sysctl -n kern.bootsessionuuid 2>/dev/null)"
    metric os_version "$(sw_vers -productVersion 2>/dev/null)"
    metric cores "$(sysctl -n hw.logicalcpu 2>/dev/null)"
    # The vm_stat queues can overlap. Use the kernel's available-memory percentage.
    memory_total="$(sysctl -n hw.memsize 2>/dev/null)" || memory_total=
    memory_percent="$(sysctl -n kern.memorystatus_level 2>/dev/null)" || memory_percent=
    case "$memory_total" in *[!0-9]*) memory_total= ;; esac
    case "$memory_percent" in *[!0-9]*) memory_percent= ;; esac
    metric memory_total "$memory_total"
    metric memory_available "$(awk -v total="$memory_total" -v percent="$memory_percent" '
      BEGIN { if(total>0 && percent!="" && percent<=100) printf "%.0f",total*percent/100 }')"
    metric load "$(sysctl -n vm.loadavg 2>/dev/null | tr -d '{}')"
    boot="$(sysctl -n kern.boottime 2>/dev/null | awk '{gsub(/,/,"",$4); print $4}')"
    metric uptime "$(awk -v now="$(date +%s)" -v boot="$boot" 'BEGIN {if(boot ~ /^[0-9]+$/ && now>=boot) printf "%.0f",now-boot}')"
    metric cpu "$(top -l 2 -s 1 -n 0 2>/dev/null | awk '/^CPU usage:/ { idle=$7; sub(/%/,"",idle); found=1 } END {if(found) printf "%.2f",100-idle}')"
    ;;
  Linux)
    metric os linux
    metric boot_id "$(cat /proc/sys/kernel/random/boot_id 2>/dev/null)"
    metric os_version "$(awk -F= '$1=="PRETTY_NAME" {v=substr($0,index($0,"=")+1); gsub(/^"|"$/,"",v); print v}' /etc/os-release 2>/dev/null)"
    metric cores "$(getconf _NPROCESSORS_ONLN 2>/dev/null)"
    metric memory_total "$(awk '/^MemTotal:/ {printf "%.0f",$2*1024}' /proc/meminfo 2>/dev/null)"
    metric memory_available "$(awk '/^MemAvailable:/ {printf "%.0f",$2*1024}' /proc/meminfo 2>/dev/null)"
    metric uptime "$(awk '{printf "%.0f",int($1)}' /proc/uptime 2>/dev/null)"
    metric load "$(awk '{print $1,$2,$3}' /proc/loadavg 2>/dev/null)"
    before="$(awk '/^cpu / {total=0; for(i=2;i<=9;i++) total+=$i; printf "%.0f %.0f",total,$5+$6}' /proc/stat 2>/dev/null)"
    sleep 1
    after="$(awk '/^cpu / {total=0; for(i=2;i<=9;i++) total+=$i; printf "%.0f %.0f",total,$5+$6}' /proc/stat 2>/dev/null)"
    metric cpu "$(awk -v before="$before" -v after="$after" 'BEGIN {if(split(before,a)==2 && split(after,b)==2 && b[1]>a[1]) printf "%.2f",100*(1-(b[2]-a[2])/(b[1]-a[1]))}')"
    ;;
  *)
    metric os other
    metric os_version "$(uname -sr)"
    ;;
esac
# df's portable form puts capacity in KiB. Preserve spaces in mount points.
df -Pk / "$HOME" 2>/dev/null | awk 'NR>1 && $2 ~ /^[0-9]+$/ {
  total=$2*1024; free=$4*1024; mount=$6; for(i=7;i<=NF;i++) mount=mount " " $i;
  printf "RFM\tdisk\t%s\t%.0f\t%.0f\n",mount,total,free
}'
printf 'RFM_END\n'
