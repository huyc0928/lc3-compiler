#!/usr/bin/env python3
"""编译器回归验证：对 tests/input/*.c 全流程验证。

流程：lc3-compile 编译 → lc3tools 汇编 → simulator 运行 →
比对 main 返回值（编译器约定经 STI RETURN_SLOT 写到 0xFDFF）。

期望值直接用系统 gcc 编译运行同一 C 程序得到（exit code 为低 8 位，
与 16 位补码返回值按低 8 位比较）。

用法:
    python3 tests/verify.py [名字过滤...]     # 跑全部或指定用例
输出每个用例 PASS/FAIL 及失败原因，最后汇总；有 FAIL 时退出码 1。
"""
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
COMPILER = os.path.join(ROOT, 'target', 'release', 'lc3-compile')
INPUT_DIR = os.path.join(HERE, 'input')
RUN_LIMIT = 200000

sys.path.insert(0, HERE)
from sim_run import assemble, Sim  # noqa: E402


def gcc_expected(c_path):
    """用 gcc 求该程序的 main 返回值（低 8 位），不可运行返回 None。"""
    with tempfile.TemporaryDirectory(prefix='gccexp-') as tmp:
        exe = os.path.join(tmp, 'prog')
        proc = subprocess.run(['gcc', '-std=c99', c_path, '-o', exe],
                              capture_output=True, text=True)
        if proc.returncode != 0:
            return None, 'gcc 编译失败: ' + proc.stderr.strip()[:200]
        run = subprocess.run([exe], capture_output=True, text=True, timeout=10)
        return run.returncode & 0xFF, None


def compile_c(c_path, out_asm):
    proc = subprocess.run([COMPILER, '-o', out_asm, c_path],
                          capture_output=True, text=True)
    return proc.returncode, proc.stdout + proc.stderr


def run_return_value(obj):
    """运行到停机，返回 0xFDFF 处的返回值。"""
    with Sim(obj) as sim:
        sim.run(RUN_LIMIT)
        return sim.read_mem(0xFDFF, 0xFDFF)[0xFDFF]


def main():
    filters = sys.argv[1:]
    cases = sorted(f for f in os.listdir(INPUT_DIR) if f.endswith('.c'))
    if filters:
        cases = [c for c in cases if any(x in c for x in filters)]

    failures = 0
    rows = []
    for case in cases:
        c_path = os.path.join(INPUT_DIR, case)
        name = case[:-2]
        notes = []

        expected, err = gcc_expected(c_path)
        if expected is None:
            rows.append((name, 'SKIP', err or 'gcc 无法运行'))
            failures += 1
            continue

        with tempfile.TemporaryDirectory(prefix='lc3c-') as tmp:
            out_asm = os.path.join(tmp, 'out.asm')
            code, output = compile_c(c_path, out_asm)
            if code != 0 or not os.path.isfile(out_asm):
                rows.append((name, 'FAIL', '编译器失败: ' + output.strip()[:200]))
                failures += 1
                continue

            obj, asm_out = assemble(out_asm)
            if obj is None:
                rows.append((name, 'FAIL', '汇编失败: ' +
                             ' '.join(asm_out.split()[-12:])))
                failures += 1
                continue

            try:
                actual = run_return_value(obj)
            except Exception as exc:  # 超时/解析失败等
                rows.append((name, 'FAIL', '运行异常: %s' % exc))
                failures += 1
                continue

        if (actual & 0xFF) == (expected & 0xFF):
            rows.append((name, 'PASS', '返回 %s' % fmt(actual)))
        else:
            rows.append((name, 'FAIL',
                         '返回 %s, C 语义应为 %s' % (fmt(actual), fmt(expected))))
            failures += 1

    for name, status, note in rows:
        print('[%s] %-28s %s' % (status, name, note))
    print('%d/%d 通过' % (len(rows) - failures, len(rows)))
    return 1 if failures else 0


def fmt(value):
    return '0x%04X (%d)' % (value & 0xFFFF, value)


if __name__ == '__main__':
    sys.exit(main())
