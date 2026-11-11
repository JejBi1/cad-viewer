#!/bin/bash
# usage: waitfor.sh logfile regex maxsec  (returns when regex found or timeout)
end=$(( $(date +%s) + $3 ))
while [ $(date +%s) -lt $end ]; do
  grep -aq -- "$2" "$1" && break
  sleep 5
done
