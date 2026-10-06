import socket
import struct
import sys
import threading


def pipe(a, b):
    try:
        while True:
            data = a.recv(65536)
            if not data:
                break
            b.sendall(data)
    except OSError:
        pass
    finally:
        for s in (a, b):
            try:
                s.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass


def handle(client, log):
    try:
        client.recv(257)
        client.sendall(b"\x05\x00")
        head = client.recv(4)
        kind = head[3]
        if kind == 1:
            host = socket.inet_ntoa(client.recv(4))
        elif kind == 3:
            host = client.recv(client.recv(1)[0]).decode()
        else:
            client.close()
            return
        port = struct.unpack(">H", client.recv(2))[0]
        log.write(f"CONNECT {host}:{port}\n")
        log.flush()
        upstream = socket.create_connection((host, port), timeout=10)
        client.sendall(b"\x05\x00\x00\x01" + socket.inet_aton("0.0.0.0") + struct.pack(">H", 0))
        threading.Thread(target=pipe, args=(client, upstream), daemon=True).start()
        pipe(upstream, client)
    except OSError:
        client.close()


server = socket.socket()
server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
server.bind(("127.0.0.1", int(sys.argv[1])))
server.listen(16)
server.settimeout(float(sys.argv[3]))
log = open(sys.argv[2], "a")
try:
    while True:
        conn, _ = server.accept()
        conn.settimeout(None)
        threading.Thread(target=handle, args=(conn, log), daemon=True).start()
except socket.timeout:
    pass
