"""A minimal IPC call to an isolated zeron engine: rpc.py <port> <data_dir> <method> <json params>."""
import base64, json, os, socket, struct, sys

port, data_dir, method, params = int(sys.argv[1]), sys.argv[2], sys.argv[3], json.loads(sys.argv[4])
assert port not in (27654, 27871), "never the production or staging engine"
token = open(os.path.join(data_dir, "ipc-token")).read().strip()
s = socket.create_connection(("127.0.0.1", port), timeout=120)
key = base64.b64encode(os.urandom(16)).decode()
s.sendall((f"GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
           f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nAuthorization: Bearer {token}\r\n\r\n").encode())
head = b""
while b"\r\n\r\n" not in head:
    head += s.recv(1)
assert b" 101 " in head.split(b"\r\n")[0], head

def send(text):
    data = text.encode()
    mask = os.urandom(4)
    n = len(data)
    hdr = bytes([0x81])
    hdr += bytes([0x80 | n]) if n < 126 else (bytes([0x80 | 126]) + struct.pack(">H", n) if n < 65536 else bytes([0x80 | 127]) + struct.pack(">Q", n))
    s.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

def recv_exact(n):
    buf = b""
    while len(buf) < n:
        chunk = s.recv(n - len(buf))
        if not chunk:
            raise EOFError
        buf += chunk
    return buf

def recv():
    message = b""
    while True:
        b0, b1 = recv_exact(2)
        n = b1 & 0x7F
        if n == 126:
            n = struct.unpack(">H", recv_exact(2))[0]
        elif n == 127:
            n = struct.unpack(">Q", recv_exact(8))[0]
        payload = recv_exact(n)
        op = b0 & 0x0F
        if op == 9:  # ping
            continue
        message += payload
        if b0 & 0x80:
            return message.decode()

send(json.dumps({"id": 1, "method": method, "params": params}))
while True:
    frame = json.loads(recv())
    if frame.get("id") == 1:
        out = os.environ.get("RPC_OUT")
        if out:
            open(out, "w").write(json.dumps(frame.get("ok")))
        print(json.dumps(frame, indent=1)[:6000])
        break
