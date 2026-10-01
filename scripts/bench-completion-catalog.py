# /// script
# dependencies = ["websockets"]
# ///
"""Time ListSkills/ListCommands against a running Zeron engine.
usage: uv run scripts/bench-completion-catalog.py <ipc-port> <chatId or -> <harness> [n] [targetDeviceId]"""
import asyncio, json, time, sys, statistics, websockets
async def main():
    port, chat, harness = sys.argv[1], sys.argv[2], sys.argv[3]
    n = int(sys.argv[4]) if len(sys.argv) > 4 else 5
    params = {"harness": harness}
    if chat != "-":
        params["chatId"] = chat
    if len(sys.argv) > 5:
        params["targetDeviceId"] = sys.argv[5]
    out = {}
    async with websockets.connect(f"ws://127.0.0.1:{port}", max_size=None) as ws:
        rid = 0
        for method in ["ListSkills", "ListCommands"]:
            times, count = [], None
            for _ in range(n):
                rid += 1
                t = time.perf_counter()
                await ws.send(json.dumps({"id": rid, "method": method, "params": params}))
                while True:
                    m = json.loads(await ws.recv())
                    if m.get("id") == rid:
                        break
                times.append(round(time.perf_counter() - t, 3))
                if "err" in m:
                    raise SystemExit(f"{method}: {m['err']}")
                count = len(m["ok"])
            out[method] = {"times_s": times, "median_s": statistics.median(times), "items": count}
    print(json.dumps({"harness": harness, "port": port, **out}))
asyncio.run(main())
