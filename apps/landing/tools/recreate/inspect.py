"""Send one command to the staged app's inspector: zi.py '<json>' [out-file-for-data]"""
import glob, json, socket, sys
path = sorted(glob.glob(f"/tmp/zgx-inspect-{__import__('os').getuid()}/*.sock"))[-1]
s = socket.socket(socket.AF_UNIX); s.settimeout(60); s.connect(path)
s.sendall((sys.argv[1] + "\n").encode())
buf = b""
while not buf.endswith(b"\n"):
    d = s.recv(1 << 22)
    if not d: break
    buf += d
r = json.loads(buf)
if len(sys.argv) > 2:
    json.dump(r.get("data"), open(sys.argv[2], "w"))
print(r.get("text") if r.get("ok") else r)
