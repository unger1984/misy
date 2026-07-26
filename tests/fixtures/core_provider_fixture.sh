#!/bin/sh

target=$1

reply() {
    printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$1" "$2"
}

while IFS= read -r line; do
    id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$line" in
        *'"method":"auth.status"'*) reply "$id" '{"authenticated":false}' ;;
        *'"method":"auth.start"'*) reply "$id" '{"url":"https://example.test/auth"}' ;;
        *'"method":"auth.complete"'*) reply "$id" '{"credentials":{"access":"opaque"}}' ;;
        *'"method":"models.list"'*) reply "$id" '{"models":[{"id":"fixture-model","display_name":"Fixture","context_window":4096}]}' ;;
        *'"method":"chat.start"'*)
            case "$line" in *'"provider_id":"fixture"'*) ;; *) printf '%s\n' '{"jsonrpc":"2.0","method":"failed","params":{"message":"missing explicit provider reference"}}'; reply "$id" '{}'; continue ;; esac
            case "$line" in *'"model_id":"fixture-model"'*) ;; *) printf '%s\n' '{"jsonrpc":"2.0","method":"failed","params":{"message":"missing explicit model reference"}}'; reply "$id" '{}'; continue ;; esac
            case "$line" in
                *'"tool_call_id":"write-1"'*)
                    printf '%s\n' '{"jsonrpc":"2.0","method":"text_delta","params":{"delta":"done"}}'
                    printf '%s\n' '{"jsonrpc":"2.0","method":"completed","params":{}}'
                    reply "$id" '{}'
                    ;;
                *'"tool_call_id":"bad-1"'*|*'"tool_call_id":"unknown-1"'*)
                    printf '%s\n' '{"jsonrpc":"2.0","method":"completed","params":{}}'
                    reply "$id" '{}'
                    ;;
                *'"tool_call_id":"loop-1"'*)
                    printf '%s\n' '{"jsonrpc":"2.0","method":"tool_call","params":{"id":"loop-1","name":"read_file","arguments":{"path":"missing"}}}'
                    printf '%s\n' '{"jsonrpc":"2.0","method":"completed","params":{}}'
                    reply "$id" '{}'
                    ;;
                *'"content":"tool-round-trip"'*)
                    printf '%s\n' '{"jsonrpc":"2.0","method":"text_delta","params":{"delta":"writing","metadata":{"stream":"one"}}}'
                    printf '{"jsonrpc":"2.0","method":"tool_call","params":{"id":"write-1","name":"write_file","arguments":{"path":"%s","content":"written by tool"}}}\n' "$target"
                    printf '{"jsonrpc":"2.0","method":"tool_call","params":{"id":"read-1","name":"read_file","arguments":{"path":"%s"}}}\n' "$target"
                    printf '%s\n' '{"jsonrpc":"2.0","method":"completed","params":{"metadata":{"finish":"tool"}}}'
                    reply "$id" '{"metadata":{"turn":"one"}}'
                    ;;
                *'"content":"bad-tool-arguments"'*)
                    printf '%s\n' '{"jsonrpc":"2.0","method":"tool_call","params":{"id":"bad-1","name":"write_file","arguments":{"path":"missing-content"}}}'
                    printf '%s\n' '{"jsonrpc":"2.0","method":"completed","params":{}}'
                    reply "$id" '{}'
                    ;;
                *'"content":"unknown-tool"'*)
                    printf '%s\n' '{"jsonrpc":"2.0","method":"tool_call","params":{"id":"unknown-1","name":"not_registered","arguments":{}}}'
                    printf '%s\n' '{"jsonrpc":"2.0","method":"completed","params":{}}'
                    reply "$id" '{}'
                    ;;
                *'"content":"provider-failure"'*)
                    printf '%s\n' '{"jsonrpc":"2.0","method":"failed","params":{"message":"fixture failure"}}'
                    reply "$id" '{}'
                    ;;
                *'"content":"cancel-me"'*) sleep 2; reply "$id" '{}' ;;
                *'"content":"turn-limit"'*)
                    printf '%s\n' '{"jsonrpc":"2.0","method":"tool_call","params":{"id":"loop-1","name":"read_file","arguments":{"path":"missing"}}}'
                    printf '%s\n' '{"jsonrpc":"2.0","method":"completed","params":{}}'
                    reply "$id" '{}'
                    ;;
                *) reply "$id" '{}' ;;
            esac
            ;;
    esac
done
