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
            printf '{"jsonrpc":"2.0","method":"text_delta","params":{"request_id":%s,"delta":"fixture"}}\n' "$id"
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
        *'"method":"test.healthy_descendant"'*)
            printf 'pid:%s\n' "$$" >> "$log_file"
            sleep 30 &
            printf 'descendant:%s\n' "$!" >> "$log_file"
            printf '{"jsonrpc":"2.0","id":%s,"result":{"healthy":true}}\n' "$id"
            ;;
        *'"method":"test.oversized_line"'*)
            # 100 MiB without a newline simulates a provider whose stdout stream never
            # terminates a protocol line; the core must reject it instead of buffering it.
            head -c 104857600 /dev/zero | tr '\0' 'x'
            ;;
        *'"method":"test.flood"'*)
            count=0
            while [ "$count" -lt 128 ]; do
                printf '{"jsonrpc":"2.0","method":"text_delta","params":{"request_id":%s,"delta":"flood"}}\n' "$id"
                count=$((count + 1))
            done
            printf '{"jsonrpc":"2.0","id":%s,"result":{"flooded":true}}\n' "$id"
            ;;
        *'"method":"test.exit"'*)
            exit 0
            ;;
        *'"method":"test.hang"'*)
            sleep 30
            ;;
        *'"method":"chat.cancel"'*'"request_id":'*)
            printf 'cancelled\n' >> "$log_file"
            ;;
    esac
done
