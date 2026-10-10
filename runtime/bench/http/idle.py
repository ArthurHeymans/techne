"""Hold N idle connections to 127.0.0.1:PORT open until killed."""

import signal
import socket
import sys

port, n = int(sys.argv[1]), int(sys.argv[2])
held = [socket.create_connection(("127.0.0.1", port)) for _ in range(n)]
print("ready", flush=True)
signal.pause()
