#!/bin/sh

target=$1

reply() {
  printf '{"jsonrpc":"2.0","id":%s,"result":' "$id"
  printf '%s' "$@"
  printf '}\n'
}

text() {
  printf '{"jsonrpc":"2.0","method":"text_delta","params":'
  printf '{"request_id":%s,"delta":"%s"}}\n' "$id" "$1"
}

complete() {
  printf '{"jsonrpc":"2.0","method":"completed","params":'
  printf '{"request_id":%s}}\n' "$id"
}

call_read() {
  printf '{"jsonrpc":"2.0","method":"tool_call","params":'
  printf '{"request_id":%s,"id":"%s","name":"read_file",' "$id" "$1"
  printf '"arguments":{"path":"%s"}}}\n' "$2"
}

call_write() {
  printf '{"jsonrpc":"2.0","method":"tool_call","params":'
  printf '{"request_id":%s,"id":"%s","name":"write_file",' "$id" "$1"
  printf '"arguments":{"path":"%s","content":"%s"}}}\n' "$2" "$3"
}

call_background_shell() {
  printf '{"jsonrpc":"2.0","method":"tool_call","params":'
  printf '{"request_id":%s,"id":"background-1","name":"exec_command",' "$id"
  printf '%s\n' '"arguments":{"cmd":"sleep 0.05; printf background-done","description":"Background fixture","run_in_background":true}}}'
}

failed() {
  printf '{"jsonrpc":"2.0","method":"failed","params":'
  printf '{"request_id":%s,"message":"%s"}}\n' "$id" "$1"
}

auth_status() {
  case "$target" in
    *slow-status*) sleep 2 ;;
    # Denies the status query even though stored credentials are injected.
    *status-unauthenticated*) reply '{"authenticated":false}' && return ;;
  esac
  case "$line" in
    *'"credentials"'*) reply '{"authenticated":true}' ;;
    *)
      reply \
        '{"authenticated":false,"credentials":{"access":"status-secret"},' \
        '"nested":{"credentials":{"access":"status-nested-secret"}}}'
      ;;
  esac
}

auth_start() {
  case "$target" in
    *slow-start*) sleep 2 ;;
  esac
  case "$target" in
    *auth-device*)
      reply \
        '{"kind":"device","url":"https://example.test/device?user_code=WDJB-MJHT",' \
        '"user_code":"WDJB-MJHT","expires_at":1795000000000,' \
        '"session":{"id":"fixture-session"}}'
      ;;
    *auth-none*) reply '{"kind":"none"}' ;;
    *auth-prompt*)
      reply \
        '{"kind":"prompt","fields":[' \
        '{"id":"api_key","label":"API key","secret":true}],' \
        '"session":{"id":"fixture-session"}}'
      ;;
    *auth-unknown*)
      reply '{"kind":"future","session":{"id":"fixture-session"}}'
      ;;
    *)
      reply \
        '{"kind":"browser","url":"https://example.test/auth",' \
        '"session":{"id":"fixture-session"},' \
        '"credentials":{"access":"start-secret"},' \
        '"nested":{"credentials":{"access":"start-nested-secret"}}}'
      ;;
  esac
}

auth_complete() {
  case "$target" in
    *auth-device*) sleep 1 ;;
    *hanging-auth*)
      if [ ! -e "${target}.complete-started" ]; then
        : > "${target}.complete-started"
        sleep 30
      fi
      ;;
  esac
  case "$line" in
    *'"session":{"id":"remote-error"}'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":' "$id"
      printf '%s' \
        '{"code":401,"message":"denied","data":' \
        '{"nested":{"credentials":{"access":"secret"}}}}}'
      printf '\n'
      ;;
    *'"session":{"id":"bearer-error"}'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":' "$id"
      printf '%s\n' \
        '{"code":401,"message":"upstream denied: authorization: Bearer sk-fixture-leaked-token-0123456789"}}'
      ;;
    *'"session":{"id":"pending-a"}'*)
      sleep 2
      reply '{"credentials":{"type":"oauth","access":"opaque"}}'
      ;;
    *'"session":{"id":"no-credentials"}'*)
      reply '{}'
      ;;
    *'"session":{"id":"expired"}'*)
      reply \
        '{"credentials":{"type":"oauth","access":"stale",' \
        '"refresh_token":"rotate","expires_at":0}}'
      ;;
    *'"session":{"id":"fixture-session"}'*)
      reply \
        '{"credentials":{"type":"oauth","access":"opaque"},' \
        '"nested":{"credentials":{"access":"complete-secret"}}}'
      ;;
    *)
      printf '{"jsonrpc":"2.0","id":%s,"error":' "$id"
      printf '%s\n' '{"code":400,"message":"missing opaque auth session"}}'
      ;;
  esac
}

models_list() {
  case "$target" in
    *async-gated-models*)
      touch "$target.started"
      (
        while [ ! -f "$target.release" ]; do sleep 0.01; done
        reply \
          '{"models":[' \
          '{"id":"fixture-model","display_name":"Fixture","context_window":4096,' \
          '"input_modalities":["text","image"]},' \
          '{"id":"fixture-model-b","display_name":"Fixture B","context_window":4096}]}'
      ) &
      return
      ;;
    *gated-models*)
      models_list_calls=$((models_list_calls + 1))
      if [ "$models_list_calls" -eq 1 ]; then
        touch "$target.first-started"
        while [ ! -f "$target.first-release" ]; do sleep 0.01; done
      else
        touch "$target.second-started"
        while [ ! -f "$target.second-release" ]; do sleep 0.01; done
      fi
      ;;
    *slow-models*|*slow-bad-models*) sleep 1 ;;
  esac
  case "$target" in
    *slow-bad-models*)
      reply '{}'
      # Cancellation tests await this marker instead of guessing the delay with sleeps.
      touch "$target.responded"
      ;;
    *bad-models*) reply '{}' ;;
    *explicit-default*)
      reply \
        '{"models":[' \
        '{"id":"fixture-model","display_name":"Fixture","context_window":4096,' \
        '"input_modalities":["text","image"]},' \
        '{"id":"fixture-model-b","display_name":"Fixture B","context_window":4096}],' \
        '"default_model":"fixture-model-b"}'
      ;;
    *)
      reply \
        '{"models":[' \
        '{"id":"fixture-model","display_name":"Fixture","context_window":4096,' \
        '"input_modalities":["text","image"]},' \
        '{"id":"fixture-model-b","display_name":"Fixture B","context_window":4096}]}'
      ;;
  esac
}

usage_get() {
  case "$line" in
    *'"credentials"'*) ;;
    *)
      printf '{"jsonrpc":"2.0","id":%s,"error":' "$id"
      printf '%s\n' '{"code":401,"message":"missing usage credentials"}}'
      return
      ;;
  esac
  case "$target" in
    *bad-usage*)
      reply \
        '{"fetched_at":1795000000000,"limits":[],' \
        '"raw":{"access_token":"must-not-escape"}}'
      ;;
    *usage-rotate*)
      reply \
        '{"fetched_at":1795000000000,"limits":[' \
        '{"id":"five-hour","label":"5 hour limit","amount":' \
        '{"used":42,"limit":100,"remaining":58,"unit":"percent"}}],' \
        '"credentials":{"type":"oauth","access":"usage-rotated",' \
        '"refresh_token":"usage-rotated-refresh"}}'
      ;;
    *)
      reply \
        '{"fetched_at":1795000000000,"limits":[' \
        '{"id":"five-hour","label":"5 hour limit","amount":' \
        '{"used":42,"limit":100,"remaining":58,"unit":"percent"},' \
        '"window":{"duration_ms":18000000,"resets_at":1795018000000}}]}'
      ;;
  esac
}

chat_start() {
  case "$line" in
    *'"provider_id":"fixture"'*|*'"provider_id":"fixture-two"'*) ;;
    *)
      failed 'missing explicit provider reference'
      reply '{}'
      return
      ;;
  esac
  case "$line" in
    # Ordered most-recent prompt first: a chat request carries the whole history, so the
    # newest message's branch must win the pattern match. The cancelled pair keeps short
    # sleeps so a queued third answer drains well inside the test deadline.
    *'"content":"image-input"'*)
      case "$line" in
        *'"attachments":[{'*) ;;
        *)
          failed 'missing image attachment'
          reply '{}'
          return
          ;;
      esac
      case "$line" in
        *'"type":"image"'*) ;;
        *)
          failed 'invalid image type'
          reply '{}'
          return
          ;;
      esac
      case "$line" in
        *'"media_type":"image/png"'*) ;;
        *)
          failed 'invalid image media type'
          reply '{}'
          return
          ;;
      esac
      case "$line" in
        *'"data_base64":"'*) ;;
        *)
          failed 'missing image data'
          reply '{}'
          return
          ;;
      esac
      case "$line" in
        *'"name":"view_image"'*) ;;
        *)
          failed 'missing view_image definition'
          reply '{}'
          return
          ;;
      esac
      text seen
      complete
      reply '{}'
      ;;
    *'"content":"queue-drain-third"'*)
      text two
      complete
      reply '{}'
      ;;
    *'"content":"queue-drain-second"'*)
      sleep 1
      complete
      reply '{}'
      ;;
    *'"content":"queue-drain-first"'*)
      sleep 1
      complete
      reply '{}'
      ;;
    *'"content":"stale-head-second"'*)
      sleep 1
      complete
      reply '{}'
      ;;
    *'"content":"stale-head-first"'*)
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"write-1"'*)
      text done
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"background-1"'*)
      text launched
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"bad-1"'*|*'"tool_call_id":"unknown-1"'*)
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"loop-1"'*)
      call_read loop-1 missing
      complete
      reply '{}'
      ;;
    *'"content":"late-next"'*)
      text next
      sleep 2
      complete
      reply '{}'
      ;;
    *'"content":"no-id-terminal"'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"completed","params":{}}'
      reply '{}'
      ;;
    *'"content":"late-cancel"'*)
      (
        sleep 1
        text late
        complete
        reply '{}'
      ) &
      ;;
    *'"content":"tool-round-trip"'*)
      text writing
      call_write write-1 "$target" 'written by tool'
      call_read read-1 "$target"
      complete
      reply '{"metadata":{"turn":"one"}}'
      ;;
    *'"content":"background-command"'*)
      call_background_shell
      complete
      reply '{}'
      ;;
    *'"content":"refresh-before-chat"'*)
      case "$line" in
        *'"access":"refreshed-opaque"'*)
          text refreshed
          complete
          reply '{}'
          ;;
        *)
          failed 'expired credentials were not refreshed'
          reply '{}'
          ;;
      esac
      ;;
    *'"content":"credential-chat"'*)
      case "$line" in
        *'"credentials":{'*'"type":"oauth"'*'"access":"opaque"'*|\
          *'"credentials":{'*'"access":"opaque"'*'"type":"oauth"'*)
          text authenticated
          complete
          reply '{}'
          ;;
        *)
          failed 'missing chat credentials'
          reply '{}'
          ;;
      esac
      ;;
    *'"content":"bad-tool-arguments"'*)
      printf '{"jsonrpc":"2.0","method":"tool_call","params":'
      printf '{"request_id":%s,"id":"bad-1","name":"write_file",' "$id"
      printf '%s\n' '"arguments":{"path":"missing-content"}}}'
      complete
      reply '{}'
      ;;
    *'"content":"unknown-tool"'*)
      printf '{"jsonrpc":"2.0","method":"tool_call","params":'
      printf '{"request_id":%s,"id":"unknown-1","name":"not_registered",' "$id"
      printf '%s\n' '"arguments":{}}}'
      complete
      reply '{}'
      ;;
    *'"content":"provider-failure"'*)
      failed 'fixture failure'
      reply '{}'
      ;;
    *'"content":"leak-bearer-error"'*)
      failed 'upstream 401: Bearer sk-fixture-leaked-chat-token-0123456789'
      reply '{}'
      ;;
    *'"content":"huge-error"'*)
      pad=$(printf '%2100s' '' | tr ' ' 'h')
      failed "portal error page: $pad"
      reply '{}'
      ;;
    *'"content":"burst"'*)
      count=0
      while [ "$count" -lt 4096 ]; do
        text x
        count=$((count + 1))
      done
      complete
      reply '{}'
      ;;
    *'"content":"continuous-stream"'*)
      while :; do
        text x
      done
      ;;
    *'"content":"session-one"'*)
      text one
      complete
      reply '{}'
      ;;
    *'"content":"session-two"'*)
      text two
      complete
      reply '{}'
      ;;
    *'"content":"block-session"'*)
      sleep 1
      complete
      reply '{}'
      ;;
    *'"content":"cancel-before-tool"'*)
      call_write cancel-write "$target" 'must not exist'
      sleep 1
      complete
      reply '{}'
      ;;
    *'"content":"turn-limit"'*)
      call_read loop-1 missing
      complete
      reply '{}'
      ;;
    *'"content":"cancel-me"'*)
      sleep 2
      reply '{}'
      ;;
    *'"content":"rotate-credentials-chat"'*)
      text rotated
      complete
      reply \
        '{"metadata":{"turn":"one"},' \
        '"credentials":{"type":"oauth","access":"chat-rotated",' \
        '"refresh_token":"chat-rotated-refresh"}}'
      ;;
    *) reply '{}' ;;
  esac
}

while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"auth.status"'*) auth_status ;;
    *'"method":"auth.start"'*) auth_start ;;
    *'"method":"auth.complete"'*) auth_complete ;;
    *'"method":"auth.refresh"'*)
      sleep 1
      case "$target" in
        # A credential-less refresh result: invalid for credentialed providers, the
        # legitimate shape for a `none`-flow provider that holds no credentials.
        *refresh-no-credentials*|*auth-none*) reply '{}' ;;
        *)
          reply \
            '{"credentials":{"type":"oauth","access":"refreshed-opaque",' \
            '"expires_at":4102444800000},' \
            '"nested":{"credentials":{"access":"refresh-secret"}}}'
          ;;
      esac
      ;;
    *'"method":"auth.logout"'*) reply '{}' ;;
    *'"method":"models.list"'*) models_list ;;
    *'"method":"usage.get"'*) usage_get ;;
    *'"method":"chat.start"'*) chat_start ;;
  esac
done
