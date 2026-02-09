port := "6969"

watch p=port:
    #!/bin/bash
    just server {{p}} &
    SERVER_PID=$!
    while inotifywait -r -e modify src Cargo.toml; do
        echo "Change detected! Restarting server..."
        kill $SERVER_PID
        just server {{p}} &
        SERVER_PID=$!
    done

server p=port:
    cargo r --bin server -- -p {{p}}

client p=port:
    cargo r --bin client -- -p {{p}}
