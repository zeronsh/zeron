#!/usr/bin/env bash
# session-pss.sh — proportional set size of a harness process tree.
#
# Usage:
#   scripts/perf/session-pss.sh [--json] [pid]
#
# With a pid, walk that process and every descendant (parent → child via
# PPid). With no pid, do that once for every running claude, codex, or grok
# that is not itself inside another one of those.
#
# A child that called setsid is still a descendant, so it is included. That
# matters for the chrome-devtools-mcp telemetry watchdog, which runs in its
# own process group. A process reparented to init is not reachable and is
# omitted. A process whose /proc entry disappears or is not readable is
# skipped; the script still exits 0.
#
# Each counted process is one group:
#   harness    argv0's basename is exactly claude, codex, or grok. The exe is
#              not enough: Claude's bundled ugrep is the claude binary with
#              argv0 `ugrep`, and that is a tool, not the session.
#   npm/npx    npm or npx, including an `npm exec …` process title
#   mcp        an MCP server: argv contains the token `mcp`, the program
#              path names an mcp server, or it is the direct child of an
#              npm/npx wrapper (the server that wrapper launched)
#   other      shells, compilers, browsers, server helpers (the watchdog)
#
# Helpers of an MCP server stay in `other`, so a watchdog is not a second
# server. PSS comes from the `Pss:` line of /proc/<pid>/smaps_rollup (kB).
# Totals are that sum / 1024, in MiB. Commands are truncated.
#
# --json prints one JSON object on stdout. Group keys are harness, mcp,
# npm, other. Human output uses the same order.
#
# Bash and awk only. SESSION_PSS_PROC overrides /proc (tests).

set -euo pipefail

PROC=${SESSION_PSS_PROC:-/proc}
CMD_MAX=160
json=0
root=

usage() {
  echo "usage: session-pss.sh [--json] [pid]" >&2
}

while [[ $# -gt 0 ]]; do
  case $1 in
    --json)
      json=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    --)
      shift
      break
      ;;
    -*)
      echo "session-pss: unknown option: $1" >&2
      usage
      exit 2
      ;;
    *)
      if [[ -n $root ]]; then
        usage
        exit 2
      fi
      if [[ ! $1 =~ ^[0-9]+$ ]]; then
        echo "session-pss: pid must be a decimal number" >&2
        usage
        exit 2
      fi
      root=$1
      shift
      ;;
  esac
done
if [[ $# -gt 0 ]]; then
  usage
  exit 2
fi

# Interpreter argv0 basenames. The following argument that is not a flag is
# the program (node path/to/server.js).
is_interpreter() {
  case $1 in
    node | nodejs | bun | deno | ruby | perl | uv | python | python2 | python3)
      return 0
      ;;
    python3.* | node-*)
      return 0
      ;;
  esac
  return 1
}

# True when this path component is an MCP server name or the `mcp` directory
# a launcher lives in (./mcp/launch.mjs, playwright-mcp, claude-code-mcp).
path_component_is_mcp() {
  local part=$1
  [[ $part == mcp || $part == mcp.exe || $part == *[-_.]mcp || $part == *[-_.]mcp.* || $part == *mcp ]]
}

scan_proc() {
  local dir pid name ppid pss line rest
  local arg raw0 base0 prog prog_base part
  local kind npm mcp_self cmd
  local -a args
  shopt -s nullglob
  for dir in "$PROC"/[0-9]*; do
    pid=${dir##*/}
    [[ $pid =~ ^[0-9]+$ ]] || continue
    [[ -r $dir/status ]] || continue

    # /proc files can look readable and then fail at open (permissions, or
    # the process vanished). The group redirect hides that and the || true
    # keeps a skip from aborting the scan.
    name=
    ppid=
    {
      while IFS= read -r line || [[ -n ${line:-} ]]; do
        case $line in
          Name:*)
            name=${line#Name:}
            name=${name#"${name%%[![:space:]]*}"}
            ;;
          PPid:*)
            ppid=${line#PPid:}
            ppid=${ppid#"${ppid%%[![:space:]]*}"}
            ppid=${ppid%%[[:space:]]*}
            ;;
        esac
      done <"$dir/status"
    } 2>/dev/null || true
    [[ $ppid =~ ^[0-9]+$ ]] || continue

    args=()
    {
      while IFS= read -r -d '' arg || [[ -n ${arg:-} ]]; do
        args+=("$arg")
      done <"$dir/cmdline"
    } 2>/dev/null || true

    raw0=
    base0=
    if ((${#args[@]} > 0)); then
      raw0=${args[0]}
      base0=${raw0##*/}
    fi

    # argv0 only. The exe of Claude's ugrep is the claude binary.
    kind=
    if [[ $base0 == claude || $base0 == codex || $base0 == grok ]]; then
      kind=$base0
    fi

    # `npm exec @playwright/mcp@latest` is one argv after npm sets the process
    # title, and that string contains a slash, so it is not a path basename.
    npm=0
    if [[ $kind == "" ]]; then
      if [[ $raw0 == npm || $raw0 == npx || $raw0 == npm\ * || $raw0 == npx\ * ]]; then
        npm=1
      elif [[ $base0 == npm || $base0 == npx ]]; then
        npm=1
      elif [[ $name == npm || $name == npx || $name == npm\ * || $name == npx\ * ]]; then
        npm=1
      fi
    fi

    prog=
    prog_base=
    if ((${#args[@]} > 0)); then
      prog=${args[0]}
      prog_base=$base0
      if is_interpreter "$prog_base"; then
        for arg in "${args[@]:1}"; do
          if [[ $arg == -* ]]; then
            continue
          fi
          prog=$arg
          prog_base=${prog##*/}
          break
        done
      fi
    fi

    mcp_self=0
    if [[ $kind == "" && $npm == 0 ]]; then
      if ((${#args[@]} > 0)); then
        for arg in "${args[@]}"; do
          if [[ $arg == mcp ]]; then
            mcp_self=1
            break
          fi
        done
      fi
      if [[ $mcp_self == 0 && -n $prog ]]; then
        if path_component_is_mcp "$prog_base"; then
          mcp_self=1
        else
          rest=$prog
          while [[ -n $rest ]]; do
            part=${rest%%/*}
            if path_component_is_mcp "$part"; then
              mcp_self=1
              break
            fi
            if [[ $rest == "$part" ]]; then
              break
            fi
            rest=${rest#*/}
          done
        fi
      fi
    fi

    pss=-
    {
      while IFS= read -r line || [[ -n ${line:-} ]]; do
        case $line in
          Pss:*)
            rest=${line#Pss:}
            rest=${rest#"${rest%%[![:space:]]*}"}
            pss=${rest%%[[:space:]]*}
            break
            ;;
        esac
      done <"$dir/smaps_rollup"
    } 2>/dev/null || true
    [[ $pss =~ ^[0-9]+$ ]] || pss=-

    cmd=
    if ((${#args[@]} > 0)); then
      cmd="${args[*]}"
    else
      cmd=$name
    fi
    cmd=${cmd//$'\t'/ }
    cmd=${cmd//$'\n'/ }
    cmd=${cmd//$'\r'/ }
    # Process titles are often space-padded (`chrome-devtools-mcp      `).
    cmd=${cmd%"${cmd##*[![:space:]]}"}
    if ((${#cmd} > CMD_MAX)); then
      cmd="${cmd:0:CMD_MAX-3}..."
    fi
    # comm can contain spaces (npm's process title). Keep the record columnar.
    name=${name//$'\t'/ }
    name=${name//$'\n'/ }

    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$pid" "$ppid" "$name" "$kind" "$pss" "$npm" "$mcp_self" "$cmd"
  done
}

scan_proc | LC_ALL=C awk -F '\t' -v json="$json" -v root="$root" '
function jesc(s,    i, c, o) {
  o = ""
  for (i = 1; i <= length(s); i++) {
    c = substr(s, i, 1)
    if (c == "\\") o = o "\\\\"
    else if (c == "\"") o = o "\\\""
    else if (c == "\n") o = o "\\n"
    else if (c == "\r") o = o "\\r"
    else if (c == "\t") o = o "\\t"
    else if (c < " ") o = o
    else o = o c
  }
  return o
}

function mib_of(kib) {
  return sprintf("%.1f", kib / 1024)
}

function classify(pid, parent_group) {
  if (kind[pid] != "") return "harness"
  if (npm[pid]) return "npm"
  if (parent_group == "npm") return "mcp"
  if (parent_group == "mcp") return "other"
  if (mcp_self[pid]) return "mcp"
  return "other"
}

function has_harness_ancestor(pid,    p, guard) {
  p = ppid[pid]
  guard = 0
  while (p != "" && p != "0" && guard < 100000) {
    if (kind[p] != "") return 1
    if (!(p in ppid)) return 0
    p = ppid[p]
    guard++
  }
  return 0
}

function walk(pid, parent_group,    group, list, one) {
  if (wstamp[pid] == gen) return
  wstamp[pid] = gen
  if (!(pid in ppid)) return
  group = classify(pid, parent_group)
  if (pss[pid] == "-") {
    skipped++
  } else {
    gcount[group]++
    glist[group, gcount[group]] = pid
    gkib[group] += pss[pid] + 0
  }
  # Peel child pids from a local string. A shared split array would be
  # overwritten by the recursive call and drop siblings.
  list = kids[pid]
  while (list != "") {
    sub(/^ +/, "", list)
    if (list == "") break
    if (match(list, /^[0-9]+/)) {
      one = substr(list, 1, RLENGTH)
      list = substr(list, RLENGTH + 1)
      walk(one, group)
    } else {
      break
    }
  }
}

# Higher PSS first. Equal PSS breaks toward the lower pid.
function worse(a, b) {
  if ((pss[a] + 0) < (pss[b] + 0)) return 1
  if ((pss[a] + 0) == (pss[b] + 0) && (a + 0) > (b + 0)) return 1
  return 0
}

function sort_group(g,    n, i, j, tmp) {
  n = gcount[g]
  for (i = 1; i <= n; i++) order[i] = glist[g, i]
  for (i = 2; i <= n; i++) {
    tmp = order[i]
    j = i - 1
    while (j >= 1 && worse(order[j], tmp)) {
      order[j + 1] = order[j]
      j--
    }
    order[j + 1] = tmp
  }
  for (i = 1; i <= n; i++) glist[g, i] = order[i]
}

function group_label(g) {
  if (g == "harness") return "harness"
  if (g == "mcp") return "mcp servers"
  if (g == "npm") return "npm/npx"
  return "other"
}

BEGIN {
  groups[1] = "harness"
  groups[2] = "mcp"
  groups[3] = "npm"
  groups[4] = "other"
  ngroups = 4
}

{
  pid = $1
  ppid[pid] = $2
  comm[pid] = $3
  kind[pid] = $4
  pss[pid] = $5
  npm[pid] = ($6 == "1")
  mcp_self[pid] = ($7 == "1")
  cmd[pid] = $8
  for (i = 9; i <= NF; i++) cmd[pid] = cmd[pid] "\t" $i
  if ($2 != "" && $2 != "0") kids[$2] = kids[$2] pid " "
}

END {
  if (root != "") {
    if (!(root in ppid)) {
      print "session-pss: pid " root " not readable" > "/dev/stderr"
    } else {
      nroots = 1
      roots[1] = root
    }
  } else {
    for (pid in kind) {
      if (kind[pid] == "") continue
      if (has_harness_ancestor(pid)) continue
      nroots++
      roots[nroots] = pid
    }
    for (i = 2; i <= nroots; i++) {
      tmp = roots[i]
      j = i - 1
      while (j >= 1 && (roots[j] + 0) > (tmp + 0)) {
        roots[j + 1] = roots[j]
        j--
      }
      roots[j + 1] = tmp
    }
    if (nroots == 0 && !json) {
      print "session-pss: no claude, codex, or grok process" > "/dev/stderr"
    }
  }

  for (r = 1; r <= nroots; r++) {
    gen++
    skipped = 0
    for (gi = 1; gi <= ngroups; gi++) {
      gcount[groups[gi]] = 0
      gkib[groups[gi]] = 0
    }
    walk(roots[r], "")
    for (gi = 1; gi <= ngroups; gi++) sort_group(groups[gi])

    total = 0
    counted = 0
    for (gi = 1; gi <= ngroups; gi++) {
      g = groups[gi]
      skib[r, g] = gkib[g] + 0
      sn[r, g] = gcount[g] + 0
      total += gkib[g] + 0
      counted += gcount[g] + 0
      for (i = 1; i <= gcount[g]; i++) spid[r, g, i] = glist[g, i]
    }
    stotal[r] = total
    scounted[r] = counted
    sskipped[r] = skipped
    sroot[r] = roots[r]
    grand += total
    grand_n += counted
  }
  nout = nroots

  if (json) {
    printf "{\n  \"sessions\": ["
    for (r = 1; r <= nout; r++) {
      pid = sroot[r]
      if (r > 1) printf ","
      printf "\n    {\n"
      printf "      \"pid\": %d,\n", pid
      printf "      \"comm\": \"%s\",\n", jesc(comm[pid])
      printf "      \"kind\": \"%s\",\n", jesc(kind[pid])
      printf "      \"command\": \"%s\",\n", jesc(cmd[pid])
      printf "      \"groups\": {\n"
      for (gi = 1; gi <= ngroups; gi++) {
        g = groups[gi]
        if (gi > 1) printf ",\n"
        printf "        \"%s\": {\n", g
        printf "          \"pss_kib\": %d,\n", skib[r, g]
        printf "          \"pss_mib\": %s,\n", mib_of(skib[r, g])
        printf "          \"processes\": ["
        if (sn[r, g] == 0) {
          printf "]\n"
        } else {
          printf "\n"
          for (i = 1; i <= sn[r, g]; i++) {
            p = spid[r, g, i]
            if (i > 1) printf ",\n"
            printf "            {\"pid\": %d, \"pss_kib\": %d, \"pss_mib\": %s, \"command\": \"%s\"}", p, pss[p] + 0, mib_of(pss[p]), jesc(cmd[p])
          }
          printf "\n          ]\n"
        }
        printf "        }"
      }
      printf "\n      },\n"
      printf "      \"total_pss_kib\": %d,\n", stotal[r]
      printf "      \"total_pss_mib\": %s,\n", mib_of(stotal[r])
      printf "      \"processes\": %d,\n", scounted[r]
      printf "      \"skipped\": %d\n", sskipped[r]
      printf "    }"
    }
    if (nout == 0) printf "\n"
    printf "],\n"
    printf "  \"total_pss_kib\": %d,\n", grand
    printf "  \"total_pss_mib\": %s,\n", mib_of(grand)
    printf "  \"processes\": %d,\n", grand_n
    printf "  \"session_count\": %d\n", nout
    printf "}\n"
    exit 0
  }

  for (r = 1; r <= nout; r++) {
    pid = sroot[r]
    label = kind[pid]
    if (label == "") label = comm[pid]
    if (r > 1) print ""
    printf "session %d %s\n", pid, label
    printf "  %s\n", cmd[pid]
    for (gi = 1; gi <= ngroups; gi++) {
      g = groups[gi]
      printf "  %-12s %4d proc %8s MiB\n", group_label(g), sn[r, g], mib_of(skib[r, g])
      for (i = 1; i <= sn[r, g]; i++) {
        p = spid[r, g, i]
        printf "    %8s %8d  %s\n", mib_of(pss[p]), p, cmd[p]
      }
    }
    printf "  %-12s %4d proc %8s MiB\n", "total", scounted[r], mib_of(stotal[r])
    if (sskipped[r] > 0) printf "  skipped %d\n", sskipped[r]
  }
  if (nout > 1) {
    print ""
    printf "all sessions %4d sessions %8s MiB\n", nout, mib_of(grand)
  }
}
'
