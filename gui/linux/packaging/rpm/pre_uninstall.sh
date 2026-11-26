if [ "$1" -eq 0 ]; then
    pkill -TERM -x phonelink-linux || true
fi
