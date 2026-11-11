#!/bin/bash
# usage: gui_smoke.sh <label> <stepview dir> <file> <PYTHONPATH or ''> [LD_PRELOAD]  -> shot_<label>.png in /root/slimocp/test/out
L=$1; SVD=$2; F=$3; export PYTHONPATH=$4; export LD_PRELOAD=$5
export HOME=/tmp/guihome_$L; rm -rf $HOME; mkdir -p $HOME
cd /root/slimocp/test
xvfb-run -a -s "-screen 0 1400x900x24" bash -c "
  /opt/venv/bin/python $SVD/main.py $F > out/gui_$L.log 2>&1 &
  PID=\$!
  sleep 25
  import -window root out/shot_$L.png
  kill \$PID; sleep 1; kill -9 \$PID 2>/dev/null; true
"
echo "log:"; tail -5 out/gui_$L.log
