#!/usr/bin/env python3
"""A TLS front for the local radosgw: listens on https://127.0.0.1:PORT with the certificate CERT/KEY and copies bytes
both ways to the gateway at 127.0.0.1:7480, so the HTTPS client can be tested against it. Used by dev/test-https.sh."""
import socket
import ssl
import sys
import threading

port, cert, key = int(sys.argv[1]), sys.argv[2], sys.argv[3]
backend = ("127.0.0.1", int(sys.argv[4]) if len(sys.argv) > 4 else 7480)


def pipe(src, dst):
    """Copies from src to dst until either closes."""
    try:
        while data := src.recv(65536):
            dst.sendall(data)
    except OSError:
        pass
    finally:
        for s in (src, dst):
            try:
                s.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass


def serve(client):
    """Connects one TLS client to the gateway."""
    try:
        upstream = socket.create_connection(backend)
    except OSError:
        client.close()
        return
    threading.Thread(target=pipe, args=(client, upstream), daemon=True).start()
    pipe(upstream, client)
    client.close()
    upstream.close()


context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(cert, key)
listener = socket.create_server(("127.0.0.1", port))
print(f"https://127.0.0.1:{port} -> http://{backend[0]}:{backend[1]}", flush=True)
while True:
    raw, _ = listener.accept()
    try:
        tls = context.wrap_socket(raw, server_side=True)
    except (ssl.SSLError, OSError):
        raw.close()
        continue
    threading.Thread(target=serve, args=(tls,), daemon=True).start()
