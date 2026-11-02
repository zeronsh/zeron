#!/usr/bin/env python3
"""Configure an owner-private native devbox; never provision or authenticate."""
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import signal
import socket
import stat
import subprocess
import sys
import time

HEADER = "# crew-devbox v1\n"


def channel():
    value = os.environ.get("CREW_CHANNEL", "production")
    require(value in ("production", "staging"), "CREW_CHANNEL must be production or staging.")
    return value


def coordinates():
    staging = channel() == "staging"
    return {
        "COMET_EDGE_URL": "https://comet-staging.internal.ashler.com" if staging else "https://comet.internal.ashler.com",
        "COMET_SCAFFOLD_URL": "https://scaffold-staging.internal.ashler.com" if staging else "https://scaffold.internal.ashler.com",
        "COMET_PROJECT_SCOPE": "ashler-staging" if staging else "ashler-production",
        "COMET_IPC_PORT": "27957" if staging else "27958",
        "COMET_HARNESS": "omp", "COMET_INFERENCE_MODE": "managed",
        "ASHLER_INCREMENTAL_TSC_CHECKS": "false",
        "ZERON_CALLBACK_PORT": "27656" if staging else "27654",
        "CHROME_PATH": str(home() / ".local/bin/crew-chromium"),
        "PUPPETEER_EXECUTABLE_PATH": str(home() / ".local/bin/crew-chromium"),
        "PLAYWRIGHT_BROWSERS_PATH": str(home() / ".local/share/crew-devbox-browser/browsers"),
        **({"COMET_DEPLOYMENT_ID": "ashler-staging"} if staging else {}),
    }


def profile_path():
    return home() / (".crew-staging" if channel() == "staging" else ".crew")


def helper_path():
    return home() / ".local/share/crew-devbox" / channel() / "bootstrap.py"


class Refusal(Exception):
    pass


def require(condition, message):
    if not condition:
        raise Refusal(message)


def home():
    path = Path(os.environ["HOME"])
    require(path.is_absolute() and path.resolve() == path and re.fullmatch(r"/[A-Za-z0-9/_.-]+", str(path)),
            "HOME must be an absolute physical path safe for native systemd coordinates.")
    return path


def config_path():
    return home() / ".config/crew-devbox" / f"{channel()}.env"


def unit_path():
    return home() / ".config/systemd/user" / f"crew-devbox-{channel()}.service"


def guard_path():
    return unit_path().with_name(unit_path().name + ".d") / "10-devbox.conf"


def guard_content():
    return (f"# crew-devbox v1\n[Service]\nUMask=0077\n"
            f"ExecStartPre=/usr/bin/env CREW_CHANNEL={channel()} /usr/bin/python3 {helper_path()} verify\n").encode()


def check_identity(config):
    provider, identity = config.get("COMET_DEVBOX_PROVIDER"), config.get("COMET_DEVBOX_ID", "")
    namespace = Path("/.namespace/tasks").is_dir()
    native = os.environ.get("BOAT_ID")
    valid = ((provider == "boat" and not namespace and native == identity
              and re.fullmatch(r"bx_[23456789abcdefghjkmnpqrstuvwxyz]{8}", identity))
             or (provider == "namespace" and namespace and not native
                 and re.fullmatch(r"[a-z0-9]{13}", identity)))
    require(sys.platform == "linux" and valid, "Run on the matching native Linux provider host; copied bindings are refused.")


def check_path(path, directory=False):
    """Never follow a snapshot helper/config symlink or another user's writable path."""
    root = home()
    require(path.is_relative_to(root), "devbox artifacts must stay inside HOME.")
    for item in (root, *reversed(path.relative_to(root).parents), path.relative_to(root)):
        candidate = item if item.is_absolute() else root / item
        if not candidate.exists() and not candidate.is_symlink():
            continue
        info = candidate.lstat()
        require(not stat.S_ISLNK(info.st_mode) and info.st_uid == os.getuid()
                and not info.st_mode & 0o022,
                f"Refusing symlink, foreign-owned, or externally writable devbox path: {candidate}")
        require(stat.S_ISDIR(info.st_mode) if candidate != path or directory else stat.S_ISREG(info.st_mode),
                f"Unexpected devbox path type: {candidate}")


def image_digest(source, size):
    """Hash exactly size bytes with one buffer capped at 1 MiB; reject length changes."""
    digest = hashlib.sha256()
    buffer = memoryview(bytearray(min(size + 1, 1024 * 1024)))
    remaining = size
    while remaining:
        count = source.readinto(buffer[:min(len(buffer), remaining)])
        if not count:
            return None
        digest.update(buffer[:count])
        remaining -= count
    return None if source.readinto(buffer[:1]) else digest.hexdigest()


def verify_binary(config):
    binary = Path(config.get("CREW_BINARY", ""))
    digest = config.get("CREW_BINARY_SHA256", "")
    require(binary.is_absolute() and re.fullmatch(r"/[A-Za-z0-9/_.-]+", str(binary))
            and binary.resolve() == binary and binary.is_file() and os.access(binary, os.X_OK)
            and not binary.stat().st_mode & 0o022,
            "CREW_BINARY must be a physical, non-shared-writable executable path safe for native systemd ExecStart.")
    check_path(binary)
    require(not any(binary.is_relative_to(home() / name / "app") for name in (".crew", ".crew-staging")),
            "Use an immutable release path outside the moving app/current installation tree.")
    require(re.fullmatch(r"[0-9a-f]{64}", digest), "Set CREW_BINARY_SHA256 to the verified lowercase SHA256.")
    with binary.open("rb", buffering=0) as source:
        require(image_digest(source, os.fstat(source.fileno()).st_size) == digest,
                "Crew binary SHA256 mismatch; nothing will be started.")



def check_profile(config, adopting=False):
    profile = profile_path()
    check_path(profile, directory=True)
    expected = config["CREW_EXISTING_DEVICE_ID"]
    if expected:
        identity = profile / "device-id"
        check_path(identity)
        require(identity.is_file() and identity.stat().st_size <= 200
                and identity.read_text().strip() == expected,
                "Existing devbox device identity differs; no profile or authentication will be copied or replaced.")
    runtime = profile / "crew-runtime.json"
    check_path(runtime)
    require(not adopting or runtime.is_file(), "Existing devbox adoption requires its saved runtime coordinates.")
    if runtime.exists():
        require(runtime.stat().st_size <= 65536, "devbox runtime configuration exceeds 64 KiB.")
        settings = json.loads(runtime.read_text())
        coordinates = {"edgeUrl": config["COMET_EDGE_URL"], "scaffoldUrl": config["COMET_SCAFFOLD_URL"],
                       "projectScope": config["COMET_PROJECT_SCOPE"], "deploymentId": config.get("COMET_DEPLOYMENT_ID"),
                       "ipcPort": int(config["COMET_IPC_PORT"])}
        require(isinstance(settings, dict) and all(settings.get(key) == value for key, value in coordinates.items()),
                "Existing devbox runtime has different deployment coordinates; nothing will be adopted.")
    override = profile / "env"
    require(not override.exists() and not override.is_symlink(),
            "Refusing a profile env override; preserve it and resolve its scope manually.")

def load_config():
    path = config_path()
    check_path(path)
    require(path.exists() and path.stat().st_mode & 0o777 == 0o600, "Configure the private devbox env first.")
    require(path.stat().st_size <= 65536, "Devbox configuration exceeds 64 KiB.")
    text = path.read_text()
    require(text.startswith(HEADER), "Refusing an unmanaged devbox env.")
    config = {}
    for line in text[len(HEADER):].splitlines():
        words = shlex.split(line)
        require(len(words) == 1 and "=" in words[0], "Invalid devbox env entry.")
        key, value = words[0].split("=", 1)
        require(key not in config, "Duplicate devbox env key.")
        config[key] = value
    expected = coordinates()
    keys = set(expected) | {"COMET_DATA_DIR", "COMET_DEVBOX_PROVIDER", "COMET_DEVBOX_ID", "ZERON_DEVICE_NAME",
                            "CREW_BINARY", "CREW_BINARY_SHA256", "CREW_EXISTING_DEVICE_ID", "CREW_PATH"}
    require(set(config) == keys and all(config[key] == value for key, value in expected.items())
            and config["COMET_DATA_DIR"] == str(profile_path()), "Devbox env has the wrong runtime scope.")
    check_identity(config)
    verify_binary(config)
    check_profile(config)
    return config


def environment(config):
    # Do not inherit another controller's IPC, bearer, authority or profile.
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("COMET_", "CREW_", "ZERON_"))}
    env.update({key: value for key, value in config.items() if not key.startswith("CREW_")})
    env["PATH"] = config["CREW_PATH"]
    return env


def command(args, env=None, acceptable=(0,)):
    # These short inspection/control commands never retain stderr or more than
    # 64 KiB stdout, including when a descendant keeps the pipe open.
    def interrupt(signum, _frame):
        raise KeyboardInterrupt(signum)

    previous = {signum: signal.signal(signum, interrupt) for signum in (signal.SIGINT, signal.SIGTERM)}
    process = None
    output = bytearray()
    try:
        process = subprocess.Popen(args, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.DEVNULL, start_new_session=True, umask=0o077)
        descriptor = process.stdout.fileno()
        os.set_blocking(descriptor, False)

        def drain():
            while True:
                try:
                    chunk = os.read(descriptor, 4096)
                except BlockingIOError:
                    return
                if not chunk:
                    return
                require(len(output) + len(chunk) <= 65536, "Command exceeded devbox's 64 KiB output limit.")
                output.extend(chunk)

        deadline = time.monotonic() + 30
        while True:
            drain()
            result = os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
            if result is not None:
                drain()  # Include bytes written immediately before leader exit.
                break
            require(time.monotonic() < deadline, "devbox inspection/control command exceeded 30 seconds.")
            time.sleep(0.02)
        status = result.si_status if result.si_code == os.CLD_EXITED else 128 + result.si_status
        require(status in acceptable,
                "devbox inspection/control command failed; inspect its status privately.")
        return status, output.decode("utf-8")
    finally:
        # Reserve the leader through the final group signal, even after repeated
        # cancellation. A reaped leader's potentially reused PGID is never signalled.
        for signum in previous:
            signal.signal(signum, signal.SIG_IGN)
        try:
            if process is not None:
                try:
                    os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
                except ChildProcessError:
                    pass
                else:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    process.wait()
                process.stdout.close()
        finally:
            for signum, handler in previous.items():
                signal.signal(signum, handler)


def systemctl(*args):
    return command(["systemctl", "--user", *args])[1]


def runtime_args(config):
    flags = (("--data-dir", "COMET_DATA_DIR"), ("--ipc-port", "COMET_IPC_PORT"),
             ("--edge-url", "COMET_EDGE_URL"), ("--scaffold-url", "COMET_SCAFFOLD_URL"),
             ("--project-scope", "COMET_PROJECT_SCOPE"), ("--deployment-id", "COMET_DEPLOYMENT_ID"))
    return [value for flag, key in flags if key in config for value in (flag, config[key])]


def crew(config, args):
    return command([config["CREW_BINARY"], *runtime_args(config), *args], environment(config))[1]


def service_state():
    status, text = command(
        ["systemctl", "--user", "show", unit_path().name, "--property=LoadState", "--property=FragmentPath",
         "--property=DropInPaths", "--property=ActiveState", "--property=MainPID"], acceptable=(0, 1),
    )
    properties = dict(line.split("=", 1) for line in text.splitlines() if "=" in line)
    load = properties.get("LoadState")
    require(load in ("loaded", "not-found") and (status == 0 or (status == 1 and load == "not-found")),
            "Cannot establish devbox user-service scope; preserve the user manager and inspect it manually.")
    dropins = properties.get("DropInPaths", "").split()
    require(dropins in ([], [str(guard_path())]),
            "Refusing unknown devbox service drop-ins; preserve and inspect them manually.")
    if dropins:
        check_path(guard_path())
        require(guard_path().read_bytes() == guard_content(), "Refusing a modified devbox binary guard.")
    active = properties.get("ActiveState")
    require(active in ("active", "inactive", "failed"), "devbox service state is unknown or transitioning; retry after it settles.")
    fragment = properties.get("FragmentPath", "")
    path = Path(fragment) if fragment else (unit_path() if unit_path().exists() or unit_path().is_symlink() else None)
    return path, active == "active", int(properties.get("MainPID") or 0)


def port_busy(port=None):
    with socket.socket() as listener:
        try:
            listener.bind(("127.0.0.1", int(port if port is not None else coordinates()["COMET_IPC_PORT"])))
            return False
        except OSError:
            return True


def owns_listener(pid, config):
    if pid <= 0:
        return False
    try:
        process = Path(f"/proc/{pid}")
        if process.stat().st_uid != os.getuid():
            return False
        # Cold restores can relocate the backing image; trust kernel image bytes, not its pathname.
        with (process / "exe").open("rb", buffering=0) as source:
            image = os.fstat(source.fileno())
            if (not stat.S_ISREG(image.st_mode) or image.st_uid != os.getuid()
                    or image.st_mode & 0o022 or image.st_size != Path(config["CREW_BINARY"]).stat().st_size
                    or image_digest(source, image.st_size) != config["CREW_BINARY_SHA256"]):
                return False
        # Retain one bounded kernel row and one FD entry, not the host's TCP table.
        # ponytail: rescan FDs per matching listener; index bounded inodes if port reuse makes this slow.
        for table in ("/proc/net/tcp", "/proc/net/tcp6"):
            with Path(table).open(encoding="ascii") as rows:
                next(rows, None)
                while line := rows.readline(4097):
                    if len(line) > 4096:
                        return False
                    fields = line.split()
                    if int(fields[1].split(":")[1], 16) != int(config["COMET_IPC_PORT"]) or fields[3] != "0A":
                        continue
                    expected = f"socket:[{fields[9]}]"
                    with os.scandir(process / "fd") as descriptors:
                        for descriptor in descriptors:
                            try:
                                if os.readlink(descriptor.path) == expected:
                                    return True
                            except FileNotFoundError:
                                continue  # An unrelated FD can close during inspection.
    except (OSError, ValueError, IndexError):
        pass
    return False


def render_unit(config):
    def escaped(value):
        return value.replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%").replace("\n", "\\n").replace("\r", "\\r")
    env = {key: value for key, value in config.items() if not key.startswith("CREW_")}
    env["PATH"] = config["CREW_PATH"]
    return ("[Unit]\nDescription=Crew devbox headless engine\nAfter=network-online.target\n"
            "ConditionEnvironment=BOAT_ID\nStartLimitIntervalSec=60\nStartLimitBurst=5\n\n[Service]\n"
            + "".join(f'Environment="{key}={escaped(value)}"\n' for key, value in env.items())
            + f'ExecStart={config["CREW_BINARY"]} {shlex.join(runtime_args(config))} headless\nRestart=on-failure\nRestartSec=5\n'
            + "\n[Install]\nWantedBy=default.target\n")


def check_service(config, owned):
    check_path(unit_path())
    path, active, pid = service_state()
    if path is not None:
        require(owned and path == unit_path(), "Refusing an unrelated devbox service.")
        check_path(path)
        require(path.read_text() == render_unit(config), "Refusing a modified devbox service.")
    else:
        require(not active, "Refusing an active service without an owned unit file.")
    require(not port_busy() or (active and owns_listener(pid, config)),
            "IPC port is occupied by an unrelated listener; nothing will be stopped.")
    return path, active


def write_new(path, content, mode):
    check_path(path)
    if path.exists():
        require(path.read_bytes() == content and path.stat().st_mode & 0o777 == mode,
                f"Refusing to replace an existing artifact: {path}")
        return
    previous_umask = os.umask(0o077)
    try:
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    finally:
        os.umask(previous_umask)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    with os.fdopen(descriptor, "wb") as target:
        target.write(content)


def device_name(config):
    """`devbox-<provider ID>`: Crew's device picker truncates long names, so the ID is
    what tells devboxes apart. Boat IDs drop their `bx_` prefix."""
    return "devbox-" + config["COMET_DEVBOX_ID"].removeprefix("bx_")


def configure():
    config = coordinates()
    # Check native identity before creating even a configuration directory.
    config["COMET_DEVBOX_PROVIDER"] = os.environ.get("COMET_DEVBOX_PROVIDER", "")
    config["COMET_DEVBOX_ID"] = os.environ.get("COMET_DEVBOX_ID", "")
    check_identity(config)
    name = os.environ.get("CREW_DEVBOX_NAME", "")
    require(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", name),
            "Set the owner-qualified devbox machine name (letters, digits, '.', '_', '-').")
    config.update(COMET_DATA_DIR=str(profile_path()),
                  ZERON_DEVICE_NAME=device_name(config),
                  CREW_BINARY=os.environ.get("CREW_BINARY", ""),
                  CREW_BINARY_SHA256=os.environ.get("CREW_BINARY_SHA256", ""),
                  CREW_PATH=f'{home()}/.local/bin:/usr/local/bin:/usr/bin:/bin')
    config["CREW_EXISTING_DEVICE_ID"] = os.environ.get("CREW_EXISTING_DEVICE_ID", "")
    require(not config["CREW_EXISTING_DEVICE_ID"] or re.fullmatch(r"[A-Za-z0-9_-]{1,200}", config["CREW_EXISTING_DEVICE_ID"]),
            "CREW_EXISTING_DEVICE_ID must be the existing devbox device-id, not a path or credential.")
    verify_binary(config)
    existing = config_path().exists()
    if existing:
        current = load_config()
        # Crew owns a registered device's name (rename it in Crew), so a re-run keeps
        # the name this box was set up with, including the older naming.
        config["ZERON_DEVICE_NAME"] = current["ZERON_DEVICE_NAME"]
        require(current == config, "Existing devbox binding differs; refusing to overwrite it.")
    else:
        profile = profile_path()
        require(not profile.exists() or config["CREW_EXISTING_DEVICE_ID"],
                "Set CREW_EXISTING_DEVICE_ID to adopt this devbox profile in place; nothing will be copied.")
        check_profile(config, adopting=profile.exists())
    boat = config["COMET_DEVBOX_PROVIDER"] == "boat"
    if boat:
        check_service(config, existing or bool(config["CREW_EXISTING_DEVICE_ID"]))
    else:
        require(not port_busy() or existing, "Refusing an occupied Namespace IPC port before configuration.")
    # The shell bootstrap may update only owner-controlled guidance/browser paths.
    check_path(home() / ".local/share/crew-devbox-browser", directory=True)
    for relative in (".local/bin/crew-chromium", ".local/bin/crew-devbox-gcloud-login", ".bashrc",
                     ".omp/agent/AGENTS.md", ".claude/CLAUDE.md", ".codex/AGENTS.md"):
        check_path(home() / relative)
    installed = helper_path()
    artifacts = {
        config_path(): ((HEADER + "".join(f"{key}={shlex.quote(value)}\n" for key, value in config.items())).encode(), 0o600),
        installed: (Path(__file__).read_bytes(), 0o700),
    }
    for source, target in (("autostart.sh", "crew-devbox-autostart"), ("keepalive.py", "crew-devbox-keepalive")):
        artifacts[home() / ".local/bin" / target] = (Path(__file__).with_name(source).read_bytes(), 0o700)
    if boat:
        artifacts[guard_path()] = (guard_content(), 0o600)
        artifacts[unit_path()] = (render_unit(config).encode(), 0o600)
    wrapper = (f"#!/bin/sh\n# crew-devbox v1\nexec env CREW_CHANNEL={channel()} "
               f"python3 {shlex.quote(str(installed))} exec \"$@\"\n")
    artifacts[home() / ".local/bin" / f"crew-devbox-{channel()}"] = (wrapper.encode(), 0o700)
    # Preflight every artifact before writing any: preserve unrelated user files.
    for path, (content, mode) in artifacts.items():
        check_path(path)
        require(not path.exists() or (path.read_bytes() == content and path.stat().st_mode & 0o777 == mode),
                f"Refusing to replace an existing artifact: {path}")
    for path, (content, mode) in artifacts.items():
        write_new(path, content, mode)
    print(f"Configured {channel()} devbox; no authentication or startup performed.")
    print(f"Start or resume: {home()}/.local/bin/crew-devbox-autostart {channel()}")
    print(f"Only if signed out, sign in interactively: {home()}/.local/bin/crew-devbox-{channel()} login")


def start_namespace(config):
    session = f"crew-engine-{channel()}"
    def tmux(*args, acceptable=(0,)):
        return command(["tmux", "-L", "crew-devbox", *args], acceptable=acceptable)
    status, _ = tmux("has-session", "-t", f"={session}", acceptable=(0, 1))
    if status:
        require(not port_busy(), "Refusing an unrelated Namespace listener; nothing will be stopped.")
        launch = f"exec env CREW_CHANNEL={channel()} python3 {shlex.quote(str(helper_path()))} run"
        created, _ = tmux("new-session", "-d", "-s", session, "-n", "engine", "-c", str(home()), launch, acceptable=(0, 1))
        if created:
            tmux("has-session", "-t", f"={session}")  # A concurrent boot shell may have created it.
    _, panes = tmux("list-panes", "-s", "-t", f"={session}", "-F", "#{window_id} #{pane_id} #{pane_pid} #{pane_dead}")
    rows = [row.split() for row in panes.splitlines()]
    require(len(rows) == 1 and len(rows[0]) == 4, "Refusing an unexpected Namespace supervisor layout.")
    window, pane, pid, dead = rows[0]
    require(not port_busy() or owns_listener(int(pid), config), "Namespace IPC listener is not the pinned engine.")
    tmux("set-option", "-w", "-t", window, "remain-on-exit", "on")
    tmux("set-hook", "-w", "-t", window, "pane-died", f"respawn-pane -t {pane}")
    if dead == "1":
        tmux("respawn-pane", "-t", pane)


def start():
    config = load_config()
    # Opposite-channel listeners are never stopped or commandeered.
    other = 27958 if channel() == "staging" else 27957
    require(not port_busy(other), "The other Crew channel is running; finish its work first.")
    boat = config["COMET_DEVBOX_PROVIDER"] == "boat"
    path, active = check_service(config, True) if boat else (None, False)
    if not active:
        status = crew(config, ("status",)).splitlines()
        modes = [line for line in status if line.startswith("Mode:")]
        accounts = [line for line in status if line.startswith("Auth:")]
        require(modes == ["Mode:     synced"] and len(accounts) == 1
                and re.fullmatch(r"Auth:     signed in as \S+(?: \(workspace [^)]+\))?", accounts[0]),
                "devbox has no recognized signed-in synced status; complete interactive crew-devbox login first.")
        check_identity(config)
        if boat:
            # Native identity is ephemeral, never snapshotted in a unit file.
            systemctl("import-environment", "BOAT_ID")
            systemctl("daemon-reload")
            command(["sudo", "-n", "loginctl", "enable-linger", os.environ["USER"]])
            systemctl("enable", "--now", unit_path().name)
        else:
            start_namespace(config)
    enabled, state = command(["systemctl", "--user", "is-enabled", "ashler-localdev.service"], acceptable=(0, 1, 3, 4))
    if enabled == 0 and state.strip() in ("enabled", "enabled-runtime"):
        systemctl("start", "ashler-localdev.service")




def main():
    action, *args = sys.argv[1:] or ["configure"]
    if action == "configure" and not args:
        configure()
    elif action == "start" and not args:
        start()
    elif action == "verify" and not args:
        load_config()  # Native automatic starts must recheck identity, scope and the pinned binary.
    elif action == "run" and not args:
        config = load_config()
        os.umask(0o077)
        os.execve(config["CREW_BINARY"], [config["CREW_BINARY"], *runtime_args(config), "headless"], environment(config))
    elif action == "exec":
        config = load_config()
        # Lifecycle changes go through the guarded start helper, never around it.
        require(args and args[0] in ("login", "logout", "status", "sync", "session", "scaffold")
                and not any(arg.split("=", 1)[0] in ("--data-dir", "--ipc-port", "--edge-url", "--scaffold-url",
                                                                   "--project-scope", "--deployment-id") for arg in args),
                "Use this wrapper only in its fixed devbox scope; use crew-devbox-autostart for startup.")
        previous_umask = os.umask(0o077)
        try:
            os.execve(config["CREW_BINARY"], [config["CREW_BINARY"], *runtime_args(config), *args], environment(config))
        finally:
            os.umask(previous_umask)  # Successful exec replaces this process; only failure returns here.
    else:
        raise Refusal("Usage: configure-host.py [configure|start|verify|run|exec COMMAND]")


if __name__ == "__main__":
    try:
        main()
    except (Refusal, OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Devbox refused: {error}", file=sys.stderr)
        sys.exit(1)
