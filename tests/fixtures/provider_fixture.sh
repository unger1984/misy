#!/bin/sh
# A deliberately language-neutral JSON-RPC/NDJSON provider fixture. It is invoked
# by the Rust integration tests through /bin/sh, not through Bun or Node.

log_file=$1
printf 'started\n' >> "$log_file"

while IFS= read -r line; do
    printf '%s\n' "$line" >> "$log_file"
    id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')

    case "$line" in
        *'"method":"chat.start"'*)
            printf '%s\n' '{"jsonrpc":"2.0","method":"text_delta","params":{"delta":"fixture"}}'
            case "$line" in
                *'"delay":"slow"'*)
                    (sleep 1; printf '{"jsonrpc":"2.0","id":%s,"result":{"reply":"slow"}}\n' "$id") &
                    ;;
                *)
                    printf '{"jsonrpc":"2.0","id":%s,"result":{"reply":"fast"}}\n' "$id"
                    ;;
            esac
            ;;
        *'"method":"models.list"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"models":[]}}\n' "$id"
            ;;
        *'"method":"test.malformed"'*)
            printf '%s\n' 'not JSON'
            ;;
        *'"method":"test.malformed_stay_alive"'*)
            printf 'pid:%s\n' "$$" >> "$log_file"
            printf '%s\n' 'not JSON'
            while :; do sleep 1; done
            ;;
        *'"method":"test.malformed_descendant"'*)
            sleep 30 &
            printf 'descendant:%s\n' "$!" >> "$log_file"
            printf '%s\n' 'not JSON'
            while :; do sleep 1; done
            ;;
        *'"method":"test.flood"'*)
            count=0
            while [ "$count" -lt 128 ]; do
                printf '%s\n' '{"jsonrpc":"2.0","method":"text_delta","params":{"delta":"flood"}}'
                count=$((count + 1))
            done
            printf '{"jsonrpc":"2.0","id":%s,"result":{"flooded":true}}\n' "$id"
            ;;
        *'"method":"test.exit"'*)
            exit 0
            ;;
        *'"method":"$/cancelRequest"'*)
            printf 'cancelled\n' >> "$log_file"
            ;;
    esac
done
