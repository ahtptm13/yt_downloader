"""Wraps a password-less minisign secret key (as stored in the
TAURI_SIGNING_PRIVATE_KEY secret) into an encrypted one with an empty
password, which is the form the Tauri bundler accepts.

Reads the key from the environment and prints the converted key.
Format: https://jedisct1.github.io/minisign/ (secret key, scrypt KDF).
"""
import base64
import hashlib
import os
import struct
import sys

OPSLIMIT = 32768
MEMLIMIT = 16 * 1024 * 1024


def scrypt_params(memlimit, opslimit):
    # same parameter choice as libsodium / rust-minisign
    opslimit = max(32768, opslimit)
    r = 8
    n_log2 = 1
    if opslimit < memlimit // 32:
        p = 1
        maxn = opslimit // (r * 4)
    else:
        maxn = memlimit // (r * 128)
    while n_log2 < 63 and (1 << n_log2) <= maxn // 2:
        n_log2 += 1
    if not opslimit < memlimit // 32:
        p = min(0x3FFFFFFF, (opslimit // 4) // (1 << n_log2)) // r
    return 1 << n_log2, r, p


def main():
    boxed = base64.b64decode(os.environ["TAURI_SIGNING_PRIVATE_KEY"]).decode()
    lines = [l for l in boxed.splitlines() if l.strip()]
    raw = base64.b64decode(lines[1])
    sig_alg, kdf_alg, chk_alg = raw[0:2], raw[2:4], raw[4:6]
    if kdf_alg == b"Sc":
        print(os.environ["TAURI_SIGNING_PRIVATE_KEY"])  # already encrypted
        return
    keynum_sk = raw[54:158]  # keynum(8) + sk(64) + checksum(32)
    salt = os.urandom(32)
    n, r, p = scrypt_params(MEMLIMIT, OPSLIMIT)
    stream = hashlib.scrypt(b"", salt=salt, n=n, r=r, p=p, maxmem=64 * 1024 * 1024, dklen=len(keynum_sk))
    enc = bytes(a ^ b for a, b in zip(keynum_sk, stream))
    out = sig_alg + b"Sc" + chk_alg + salt + struct.pack("<Q", OPSLIMIT) + struct.pack("<Q", MEMLIMIT) + enc
    text = "untrusted comment: minisign encrypted secret key\n" + base64.b64encode(out).decode() + "\n"
    print(base64.b64encode(text.encode()).decode())


if __name__ == "__main__":
    sys.exit(main())
