#!/usr/bin/env python3
"""用 pty 驱动 lc3tools 的 simulator，供回归与 Lab 验证复用。

直接管道喂命令不可靠：simulator 运行期间键盘设备会抢走 stdin 里待读的
输入（见 lc3-isa.md 常见陷阱 8），所以这里开伪终端，一条命令一个来回。

用法（作为模块）:
    from sim_run import assemble, Sim
    obj = assemble('out.asm')
    with Sim(obj) as sim:
        sim.set_mem(0x30FF, 3)
        sim.run(limit=50000)
        mem = sim.read_mem(0x3100, 0x3108)

用法（命令行，临时排查）:
    python3 sim_run.py prog.obj "set 0x30FF 3" "run 50000" "mem 0x3100 0x3108"
"""
import os
import pty
import re
import select
import subprocess
import sys
import tempfile
import time

TOOLS = os.environ.get(
    'LC3TOOLS_BIN', os.path.expanduser('~/projects/ics/lc3tools/build/bin'))
ASSEMBLER = os.path.join(TOOLS, 'assembler')
SIMULATOR = os.path.join(TOOLS, 'simulator')


def assemble(asm_path, workdir=None):
    """用 lc3tools 汇编，成功返回 .obj 路径，失败返回 (None, 输出)。"""
    asm_path = os.path.abspath(asm_path)
    proc = subprocess.run([ASSEMBLER, asm_path, '--print-level=3'],
                          capture_output=True, text=True)
    obj = asm_path[:-4] + '.obj' if asm_path.endswith('.asm') else asm_path + '.obj'
    if proc.returncode == 0 and os.path.isfile(obj):
        return obj, proc.stdout + proc.stderr
    return None, proc.stdout + proc.stderr


class Sim:
    """一个 simulator 实例；可重复 run 多个用例（先 restart 再 set）。"""

    def __init__(self, obj):
        self.master, slave = pty.openpty()
        self.proc = subprocess.Popen(
            [SIMULATOR, obj],
            stdin=slave, stdout=slave, stderr=slave,
            close_fds=True,
        )
        os.close(slave)
        self.wait_prompt()

    def _read(self, timeout):
        ready, _, _ = select.select([self.master], [], [], timeout)
        if not ready:
            raise TimeoutError('等待 simulator 输出超时')
        try:
            return os.read(self.master, 65536)
        except OSError:
            return b''

    def wait_prompt(self, timeout=10.0):
        out = b''
        deadline = time.time() + timeout
        while time.time() < deadline:
            out += self._read(max(0.05, deadline - time.time()))
            if out.endswith(b'> '):
                return out.decode('utf-8', 'replace')
        raise TimeoutError('没有等到提示符，最后输出:\n' + out.decode('utf-8', 'replace'))

    def send(self, command, timeout=10.0):
        os.write(self.master, command.encode() + b'\n')
        return self.wait_prompt(timeout)

    def restart(self):
        """清空状态重载（restart 会清内存，需要重新 load）。"""
        self.send('restart')
        self.send('load ' + self.obj_path)

    def set_mem(self, addr, value):
        self.send('set 0x%04X 0x%04X' % (addr, value & 0xFFFF))

    def set_reg(self, reg, value):
        self.send('set r%d 0x%04X' % (reg, value & 0xFFFF))

    def set_pc(self, value):
        self.send('set pc 0x%04X' % (value & 0xFFFF))

    def run(self, limit=0):
        """运行到停机；limit>0 时限制指令数防死循环。"""
        return self.send('run %d' % limit if limit else 'run')

    def read_mem(self, start, end):
        """读 [start, end] 闭区间，返回 {addr: value}。"""
        out = self.send('mem 0x%04X 0x%04X' % (start, end))
        mem = {}
        for m in re.finditer(r'0x([0-9A-F]{4}): 0x([0-9A-F]{4})', out):
            mem[int(m.group(1), 16)] = int(m.group(2), 16)
        if len(mem) != end - start + 1:
            raise RuntimeError('内存读取不完整:\n' + out)
        return mem

    def read_reg(self, reg):
        out = self.send('regs')
        m = re.search(r'R%d: 0x([0-9A-F]{4})' % reg, out)
        if not m:
            raise RuntimeError('无法解析寄存器:\n' + out)
        return int(m.group(1), 16)

    def close(self):
        try:
            self.send('quit')
        except (OSError, TimeoutError):
            pass
        finally:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
            os.close(self.master)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()


def load_and_run(obj, preloads=None, limit=0, mem_windows=()):
    """一次性帮助函数：预载内存、运行、读回若干窗口。

    preloads: [(addr, value), ...]
    mem_windows: [(start, end), ...]
    返回 (run 输出, {start: {addr: value}})
    """
    with Sim(obj) as sim:
        for addr, value in (preloads or []):
            sim.set_mem(addr, value)
        out = sim.run(limit)
        windows = {s: sim.read_mem(s, e) for s, e in mem_windows}
    return out, windows


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    obj = sys.argv[1]
    cmds = sys.argv[2:]
    with Sim(obj) as sim:
        for c in cmds:
            print('> ' + c)
            print(sim.send(c), end='')
    return 0


if __name__ == '__main__':
    sys.exit(main())
