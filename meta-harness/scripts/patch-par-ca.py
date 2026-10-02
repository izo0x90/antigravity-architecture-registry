#!/usr/bin/env python3
"""Sandbox only: add a CA certificate to the CA bundles inside agy_acp_server.par.

The Docker Sandboxes proxy re-signs HTTPS with its own CA. The ACP server ignores
SSL_CERT_FILE / REQUESTS_CA_BUNDLE for some calls and trusts only the bundles packed
in the .par (certifi, gRPC roots), so onboarding fails with CERTIFICATE_VERIFY_FAILED.

The .par is a launcher with a zip archive appended; the bundles are stored
uncompressed. Each one is rewritten in place at the same size (the CA goes first,
trailing roots are dropped to make room, the rest is padded with a PEM comment) and
its CRC-32 is updated in the local header and central directory. Idempotent.

Usage: patch-par-ca.py <agy_acp_server.par> <ca.pem>
"""
import re
import struct
import sys
import zipfile
import zlib

BUNDLES = (
    "google3/third_party/py/certifi/cacert.pem",
    "google3/third_party/py/httplib2/cacerts.txt",  # googleapiclient's transport
    "google3/net/grpc/python/_cython/_credentials/roots.pem",
    "google3/third_party/py/grpc/_cython/_credentials/roots.pem",
)
CERT_RE = re.compile(rb"-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----\n?", re.S)


def central_directory_crc_offsets(f, archive: zipfile.ZipFile) -> dict:
    """Maps entry name -> absolute file offset of its CRC-32 in the central directory."""
    offsets = {}
    pos = archive.start_dir  # absolute offset, already corrected for the launcher prefix
    for _ in archive.infolist():
        f.seek(pos)
        record = f.read(46)
        if record[:4] != b"PK\x01\x02":
            raise SystemExit("unexpected central directory layout")
        name_len, extra_len, comment_len = struct.unpack("<HHH", record[28:34])
        name = f.read(name_len).decode("utf-8", "replace")
        offsets[name] = pos + 16
        pos += 46 + name_len + extra_len + comment_len
    return offsets


def same_size_bundle(original: bytes, ca: bytes) -> bytes:
    """CA first, then as many original certs as fit, padded to the original size."""
    if ca in original:
        return original
    certs = CERT_RE.findall(original)
    out = ca
    for cert in certs:
        if len(out) + len(cert) + 2 > len(original):
            break
        out += cert
    pad = len(original) - len(out)
    if pad < 2:
        raise SystemExit("bundle too small to make room for the CA")
    return out + b"#" * (pad - 1) + b"\n"


def main(par_path: str, ca_path: str) -> None:
    ca = open(ca_path, "rb").read()
    if not ca.endswith(b"\n"):
        ca += b"\n"
    archive = zipfile.ZipFile(par_path)
    with open(par_path, "r+b") as f:
        crc_offsets = central_directory_crc_offsets(f, archive)
        for name in BUNDLES:
            try:
                info = archive.getinfo(name)
            except KeyError:
                print(f"skip (absent): {name}")
                continue
            if info.compress_type != zipfile.ZIP_STORED or info.flag_bits & 0x08:
                raise SystemExit(f"{name}: not a plain stored entry; refusing to patch")
            original = archive.read(name)
            patched = same_size_bundle(original, ca)
            if patched == original:
                print(f"already patched: {name}")
                continue
            assert len(patched) == len(original)
            f.seek(info.header_offset)
            local = f.read(30)
            if local[:4] != b"PK\x03\x04":
                raise SystemExit(f"{name}: bad local header")
            name_len, extra_len = struct.unpack("<HH", local[26:30])
            data_offset = info.header_offset + 30 + name_len + extra_len
            crc = struct.pack("<I", zlib.crc32(patched) & 0xFFFFFFFF)
            f.seek(data_offset)
            f.write(patched)
            f.seek(info.header_offset + 14)
            f.write(crc)
            f.seek(crc_offsets[name])
            f.write(crc)
            print(f"patched: {name}")
    # Verify: every bundle reads back with a valid CRC and starts with the CA.
    with zipfile.ZipFile(par_path) as check:
        for name in BUNDLES:
            if name in check.namelist():
                assert check.read(name).startswith(ca), name
    print("verified")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    main(sys.argv[1], sys.argv[2])
