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

call_tool() {
  printf '{"jsonrpc":"2.0","method":"tool_call","params":'
  printf '{"request_id":%s,"id":"%s","name":"%s",' "$id" "$1" "$2"
  printf '"arguments":%s}}\n' "$3"
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
  case "$target" in
    *auth-prompt-reflect*)
      printf '{"jsonrpc":"2.0","id":%s,"error":' "$id"
      printf '%s\n' '{"code":401,"message":"rejected API key sk-reflected"}}'
      return
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
    *'"tool_call_id":"scope-write-1"'*)
      if [ "${scope_retry_seen:-0}" -eq 0 ]; then
        case "$line" in *'global rules'*'root rules'*) ;; *) failed 'base AGENTS.md instructions missing'; reply '{}'; continue ;; esac
        case "$line" in *'frontend rules'*) ;; *) failed 'frontend AGENTS.md instructions missing'; reply '{}'; continue ;; esac
        case "$line" in *'backend rules'*) ;; *) failed 'backend AGENTS.md instructions missing'; reply '{}'; continue ;; esac
        scope_retry_seen=1
        while [ ! -f "$target/allow-retry" ]; do sleep 0.01; done
        call_write scope-write-1 "$target/frontend/instruction-preflight.txt" frontend
        call_write scope-write-2 "$target/backend/instruction-preflight.txt" backend
        complete
        reply '{}'
      else
        text scoped-done
        complete
        reply '{}'
      fi
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
    *'"tool_call_id":"question-1"'*)
      text question-answered
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"child-question"'*)
      text child-question-answer
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"spawn-child-question"'*)
      text parent-question-observed
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"child-todo-query"'*)
      text child-todo-answer
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"child-todo-update"'*)
      call_tool child-todo-query SetTodoList '{}'
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"spawn-child-todo"'*)
      text parent-todo-observed
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"root-todo"'*)
      case "$line" in
        *'"content":"child-todo-task"'*)
          call_tool child-todo-update SetTodoList \
            '{"todos":[{"title":"Child item","status":"done"}]}'
          ;;
        *)
          call_tool spawn-child-todo spawn_agent \
            '{"task":"child-todo-task","description":"Todo child","run_in_background":false}'
          ;;
      esac
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"dismiss-'*)
      dismiss_round=$((dismiss_round + 1))
      if [ "$dismiss_round" -le 4 ]; then
        call_tool "dismiss-$dismiss_round" AskUserQuestion \
          '{"questions":[{"question":"Continue?","options":[{"label":"Yes"},{"label":"No"}]}]}'
        complete
      else
        text dismiss-limit-complete
        complete
      fi
      reply '{}'
      ;;
    *'"tool_call_id":"todo-query"'*)
      text todo-complete
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"todo-update"'*)
      call_tool todo-query SetTodoList '{}'
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
    *'"content":"instruction-preflight"'*)
      call_write scope-write-1 "$target/frontend/instruction-preflight.txt" frontend
      call_write scope-write-2 "$target/backend/instruction-preflight.txt" backend
      complete
      reply '{}'
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
    *'"content":"question-round-trip"'*)
      call_tool question-1 AskUserQuestion \
        '{"questions":[{"question":"Choose a mode","header":"Mode","options":[{"label":"Safe","description":"Use guarded behavior."},{"label":"Fast","description":"Use optimistic behavior."}],"multi_select":false}]}'
      complete
      reply '{}'
      ;;
    *'"content":"child-question-task"'*)
      call_tool child-question AskUserQuestion \
        '{"questions":[{"question":"Child choice","options":[{"label":"A"},{"label":"B"}]}]}'
      complete
      reply '{}'
      ;;
    *'"content":"child-todo-task"'*)
      call_tool child-todo-update SetTodoList \
        '{"todos":[{"title":"Child item","status":"done"}]}'
      complete
      reply '{}'
      ;;
    *'"content":"spawn-child-todo"'*)
      call_tool root-todo SetTodoList \
        '{"todos":[{"title":"Root item","status":"in_progress"}]}'
      complete
      reply '{}'
      ;;
    *'"content":"spawn-child-question"'*)
      call_tool spawn-child-question spawn_agent \
        '{"task":"child-question-task","description":"Question child","run_in_background":false}'
      complete
      reply '{}'
      ;;
    *'"content":"question-dismiss-limit"'*)
      dismiss_round=1
      call_tool dismiss-1 AskUserQuestion \
        '{"questions":[{"question":"Continue?","options":[{"label":"Yes"},{"label":"No"}]}]}'
      complete
      reply '{}'
      ;;
    *'"content":"todo-round-trip"'*)
      call_tool todo-update SetTodoList \
        '{"todos":[{"title":"Implement lifecycle","status":"done"}]}'
      complete
      reply '{}'
      ;;
    *'"content":"question-capability-gate"'*)
      case "$line" in
        *'"name":"AskUserQuestion"'*) failed 'question tool leaked without capability' ;;
        *) text capability-hidden; complete ;;
      esac
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
    *'"tool_call_id":"wait-message"'*)
      text message-observed
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"message-1"'*)
      touch "$target.message-accepted"
      call_tool wait-message agent_wait '{"agent_ids":["agent-1"],"timeout_ms":300000}'
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"spawn-message"'*)
      call_tool message-1 agent_message '{"agent_id":"agent-1","message":"follow-up child"}'
      complete
      reply '{}'
      ;;
    *'"content":"follow-up child"'*)
      text follow-up-answer
      complete
      reply '{}'
      ;;
    *'"content":"child-message-task"'*)
      (
        while [ ! -f "$target.message-accepted" ]; do sleep 0.01; done
        text first-answer
        complete
        reply '{}'
      ) &
      ;;
    *'"content":"spawn-message"'*)
      call_tool spawn-message spawn_agent \
        '{"task":"child-message-task","description":"Message child","run_in_background":true}'
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"wait-background"'*)
      text background-observed
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"spawn-background"'*)
      call_tool wait-background agent_wait '{"agent_ids":["agent-1"],"timeout_ms":300000}'
      complete
      reply '{}'
      ;;
    *'"content":"child-background-task"'*)
      sleep 1
      text child-background-answer
      complete
      reply '{}'
      ;;
    *'"content":"spawn-background"'*)
      call_tool spawn-background spawn_agent \
        '{"task":"child-background-task","description":"Background child","run_in_background":true}'
      complete
      reply '{}'
      ;;
    *'"tool_call_id":"spawn-sync"'*)
      text sync-observed
      complete
      reply '{}'
      ;;
    *'"content":"child-sync-task"'*)
      case "$line" in
        *'"name":"spawn_agent"'*)
          failed 'child received agent tool definitions'
          reply '{}'
          return
          ;;
      esac
      text child-sync-answer
      complete
      reply '{}'
      ;;
    *'"content":"spawn-sync"'*)
      call_tool spawn-sync spawn_agent \
        '{"task":"child-sync-task","description":"Sync child","run_in_background":false}'
      complete
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
