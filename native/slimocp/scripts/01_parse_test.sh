cd /root/slimocp/work/linux && source /root/slimocp/scripts/env_gen.sh
time $PY -m bindgen -n 8 $PYWRAP_ARGS parse /root/slimocp/work/ocp_test.toml tmp_test.pkl Linux
