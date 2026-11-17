#!/usr/bin/env python3
"""Writes large fuzz seeds (megabyte files, 100,000-deep nesting) to OUT/<target>/.

Too big to check in; see docs/fuzzing.md, "Large inputs". Usage: large-seeds.py OUT
"""
import random
import sys
from pathlib import Path

out = Path(sys.argv[1])


def w(target, name, data):
    (out / target).mkdir(parents=True, exist_ok=True)
    (out / target / name).write_bytes(data)


# Selector bytes: Language::ALL order (0 Rust, 1 Swift, 2 TypeScript, 4 JavaScript, 5 Python,
# 6 Java).
min_ts = b"var a=function(b){return b+1};" * (300_000 // 30)
rust_lines = b"".join(b"fn f%d() { let x = %d; }\n" % (i, i) for i in range(15000))
deep_rust = b"fn f() " + b"{" * 100000 + b"}" * 100000
for t in ["highlight", "outline"]:
    w(t, "min_ts", b"\x02" + min_ts)
    w(t, "rust_lines", b"\x00" + rust_lines)
    w(t, "deep_rust", b"\x00" + deep_rust)
    w(t, "deep_js", b"\x04x=" + b"(" * 100000 + b"1" + b")" * 100000)
    w(t, "deep_py", b"\x05x=" + b"[" * 100000 + b"]" * 100000)
    w(t, "swift_nest", b"\x01" + b"struct S { " * 20000 + b"}" * 20000)
    w(t, "java_nest", b"\x06" + b"class C { " * 20000 + b"}" * 20000)
    w(t, "java_chain", b"\x06class C { Object o = a" + b".b()" * 50000 + b"; }")
head = rust_lines.replace(b"fn f777()", b"fn g777()").replace(b"let x = 9000;", b"let x = 1;")
w("changes", "big_rust", b"\x00" + rust_lines + b"\xff" + head + b"fn z() {}\n")
w("changes", "big_worktree", b"\x80" + rust_lines + b"\xff" + head + b"\xff" + rust_lines)
w("changes", "deep", b"\x00" + deep_rust + b"\xff" + deep_rust.replace(b"{{{", b"{x{", 1))
lines = b"".join(b"line %d\n" % i for i in range(40000))
random.seed(1)
shuffled = b"".join(b"line %d\n" % random.randrange(40000) for _ in range(40000))
w("line_diff", "big_similar", b"\x00" + lines + b"\xff" + lines.replace(b"line 20000", b"LINE"))
w("line_diff", "big_different", b"\x00" + lines + b"\xff" + shuffled)
w("source_buffer", "crlf_big", b"ab\xc3\xa9\r\n" * 100000)
w("source_buffer", "one_line", b"x\xf0\x9f\x98\x80" * 100000)
