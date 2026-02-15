port := "6969"

alias w:=watch
alias s:=server
alias c:=client
alias a:=autobahn

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

autobahn:
    just server 9001 --features=autobahn

server p=port args="":
    cargo r --bin server {{args}} -- -p {{p}}

client p=port:
    cargo r --bin client -- -p {{p}}

flame:
    RUSTFLAGS="-C force-frame-pointers=yes" cargo b --release
    sudo flamegraph --freq 999  -- ./target/release/server -p 6969
