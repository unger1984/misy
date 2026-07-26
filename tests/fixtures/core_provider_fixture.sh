#!/bin/sh

target=$1
reply() { printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$1"; }
text() { printf '{"jsonrpc":"2.0","method":"text_delta","params":{"request_id":%s,"delta":"%s"}}\n' "$id" "$1"; }
complete() { printf '{"jsonrpc":"2.0","method":"completed","params":{"request_id":%s}}\n' "$id"; }
call_read() { printf '{"jsonrpc":"2.0","method":"tool_call","params":{"request_id":%s,"id":"%s","name":"read_file","arguments":{"path":"%s"}}}\n' "$id" "$1" "$2"; }
call_write() { printf '{"jsonrpc":"2.0","method":"tool_call","params":{"request_id":%s,"id":"%s","name":"write_file","arguments":{"path":"%s","content":"%s"}}}\n' "$id" "$1" "$2" "$3"; }

while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"auth.status"'*) reply '{"authenticated":false,"credentials":{"access":"status-secret"},"nested":{"credentials":{"access":"status-nested-secret"}}}' ;;
    *'"method":"auth.start"'*) reply '{"url":"https://example.test/auth","credentials":{"access":"start-secret"},"nested":{"credentials":{"access":"start-nested-secret"}}}' ;;
    *'"method":"auth.complete"'*'"remote-error"'*) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":401,"message":"denied","data":{"nested":{"credentials":{"access":"secret"}}}}}\n' "$id" ;;
    *'"method":"auth.complete"'*) reply '{"credentials":{"access":"opaque"},"nested":{"credentials":{"access":"complete-secret"}}}' ;;
    *'"method":"auth.refresh"'*) sleep 1; reply '{"credentials":{"access":"refreshed-opaque"},"nested":{"credentials":{"access":"refresh-secret"}}}' ;;
    *'"method":"auth.logout"'*) reply '{}' ;;
    *'"method":"models.list"'*) reply '{"models":[{"id":"fixture-model","display_name":"Fixture","context_window":4096},{"id":"fixture-model-b","display_name":"Fixture B","context_window":4096}]}' ;;
    *'"method":"chat.start"'*)
      case "$line" in *'"provider_id":"fixture"'*|*'"provider_id":"fixture-two"'*) ;; *) printf '{"jsonrpc":"2.0","method":"failed","params":{"request_id":%s,"message":"missing explicit provider reference"}}\n' "$id"; reply '{}'; continue ;; esac
      case "$line" in
        *'"tool_call_id":"write-1"'*) text done; complete; reply '{}' ;;
        *'"tool_call_id":"bad-1"'*|*'"tool_call_id":"unknown-1"'*) complete; reply '{}' ;;
        *'"tool_call_id":"loop-1"'*) call_read loop-1 missing; complete; reply '{}' ;;
        *'"content":"late-next"'*) text next; sleep 2; complete; reply '{}' ;;
        *'"content":"no-id-terminal"'*) printf '%s\n' '{"jsonrpc":"2.0","method":"completed","params":{}}'; reply '{}' ;;
        *'"content":"late-cancel"'*) (sleep 1; printf '{"jsonrpc":"2.0","method":"text_delta","params":{"request_id":%s,"delta":"late"}}\n' "$id"; complete; reply '{}') & ;;
        *'"content":"tool-round-trip"'*) text writing; call_write write-1 "$target" 'written by tool'; call_read read-1 "$target"; complete; reply '{"metadata":{"turn":"one"}}' ;;
        *'"content":"credential-chat"'*) case "$line" in *'"credentials":{"access":"opaque"}'*) text authenticated; complete; reply '{}' ;; *) printf '{"jsonrpc":"2.0","method":"failed","params":{"request_id":%s,"message":"missing chat credentials"}}\n' "$id"; reply '{}' ;; esac ;;
        *'"content":"bad-tool-arguments"'*) printf '{"jsonrpc":"2.0","method":"tool_call","params":{"request_id":%s,"id":"bad-1","name":"write_file","arguments":{"path":"missing-content"}}}\n' "$id"; complete; reply '{}' ;;
        *'"content":"unknown-tool"'*) printf '{"jsonrpc":"2.0","method":"tool_call","params":{"request_id":%s,"id":"unknown-1","name":"not_registered","arguments":{}}}\n' "$id"; complete; reply '{}' ;;
        *'"content":"provider-failure"'*) printf '{"jsonrpc":"2.0","method":"failed","params":{"request_id":%s,"message":"fixture failure"}}\n' "$id"; reply '{}' ;;
        *'"content":"burst"'*) count=0; while [ "$count" -lt 4096 ]; do text x; count=$((count+1)); done; complete; reply '{}' ;;
        *'"content":"continuous-stream"'*) while :; do text x; done ;;
        *'"content":"session-one"'*) text one; complete; reply '{}' ;;
        *'"content":"session-two"'*) text two; complete; reply '{}' ;;
        *'"content":"block-session"'*) sleep 1; complete; reply '{}' ;;
        *'"content":"cancel-before-tool"'*) call_write cancel-write "$target" 'must not exist'; sleep 1; complete; reply '{}' ;;
        *'"content":"turn-limit"'*) call_read loop-1 missing; complete; reply '{}' ;;
        *'"content":"cancel-me"'*) sleep 2; reply '{}' ;;
        *) reply '{}' ;;
      esac ;;
  esac
done
