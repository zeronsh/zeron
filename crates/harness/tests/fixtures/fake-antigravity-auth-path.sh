#!/bin/sh

[ "$GEMINI_HOME" = "$ZERON_TEST_EXPECTED_GEMINI_HOME" ] || exit 1
[ -f "$GEMINI_HOME/antigravity-acp/settings.json" ] || exit 1

while read -r line; do
  id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"id":%s,"result":{"protocolVersion":1,"authMethods":[{"id":"oauth-personal"},{"id":"oauth-business"}]}}\n' "$id"
      ;;
    *'"method":"authenticate"'*)
      case "$line" in
        *'"methodId":"oauth-business"'*) printf '{"id":%s,"result":{}}\n' "$id" ;;
        *) printf '{"id":%s,"error":{"code":-32602,"message":"configured business auth was not preserved"}}\n' "$id" ;;
      esac
      ;;
  esac
done
