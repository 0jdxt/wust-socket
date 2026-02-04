watch:
    #!/bin/bash
    just server &
    SERVER_PID=$!
    while inotifywait -r -e modify src Cargo.toml; do
        echo "Change detected! Restarting server..."
        kill $SERVER_PID
        just server &
        SERVER_PID=$!
    done

server:
    cargo r --bin server -- -p 6969

client:
    cargo r --bin client -- -p 6969
