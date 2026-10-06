import socket
import sys
import time


def send(host, port, count, size):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sent = 0
    for _ in range(count):
        try:
            s.sendto(b"x" * size, (host, port))
            sent += 1
        except OSError:
            pass
        time.sleep(0.01)
    print(f"udp-sent={sent}")


def listen(port, path, quiet):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(("127.0.0.1", port))
    s.settimeout(quiet)
    total = 0
    with open(path, "w") as out:
        out.write("0")
    try:
        while True:
            data, _ = s.recvfrom(65536)
            total += len(data)
            with open(path, "w") as out:
                out.write(str(total))
    except socket.timeout:
        pass


def ask(server):
    query = bytes.fromhex("123401000001000000000000") + b"\x09localhost\x00\x00\x01\x00\x01"
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.settimeout(2)
    try:
        s.connect((server, 53))
        s.send(query)
        s.recv(512)
        print("dns=answered")
    except OSError:
        print("dns=closed")


def resolver():
    servers = []
    with open("/etc/resolv.conf") as conf:
        for line in conf:
            parts = line.split()
            if len(parts) >= 2 and parts[0] == "nameserver" and ":" not in parts[1]:
                servers.append(parts[1])
    if servers:
        ask(servers[0])
    else:
        print("dns=closed")


kind = sys.argv[1]
if kind == "send":
    send(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5]))
elif kind == "listen":
    listen(int(sys.argv[2]), sys.argv[3], float(sys.argv[4]))
elif kind == "ask":
    ask(sys.argv[2])
elif kind == "resolver":
    resolver()
