#!/usr/bin/env python3
"""
SSE Gateway 压力测试脚本
一次性创建大量 SSE 连接用于测试

用法:
    python stress_test.py                          # 默认1万连接
    python stress_test.py --count 5000             # 5000个连接
    python stress_test.py --url http://localhost:8080  # 指定服务URL
    python stress_test.py --duration 60            # 保持连接60秒
"""

import asyncio
import aiohttp
import argparse
import time
import signal
import sys
import ssl
from dataclasses import dataclass, field
from typing import Optional, Dict
from collections import defaultdict
import random
import string


@dataclass
class Stats:
    """连接统计"""
    total: int = 0
    connected: int = 0
    failed: int = 0
    disconnected: int = 0
    messages_received: int = 0
    start_time: float = 0
    errors: Dict[str, int] = field(default_factory=lambda: defaultdict(int))
    
    def __str__(self):
        elapsed = time.time() - self.start_time if self.start_time else 0
        rate = self.connected / elapsed if elapsed > 0 else 0
        return (
            f"总数: {self.total} | "
            f"已连接: {self.connected} | "
            f"失败: {self.failed} | "
            f"断开: {self.disconnected} | "
            f"消息: {self.messages_received} | "
            f"速率: {rate:.1f}/s"
        )
    
    def add_error(self, error: Exception):
        """记录错误类型"""
        error_type = type(error).__name__
        # 提取更详细的错误信息
        error_msg = str(error)
        if "Connect call failed" in error_msg or "Cannot connect" in error_msg:
            key = f"连接失败: {error_type}"
        elif "Timeout" in error_type or "timeout" in error_msg.lower():
            key = "连接超时"
        elif "ConnectionReset" in error_type or "Connection reset" in error_msg:
            key = "连接被重置"
        elif "ServerDisconnected" in error_type:
            key = "服务器断开连接"
        elif "ClientConnectorError" in error_type:
            if "Connection refused" in error_msg:
                key = "连接被拒绝"
            elif "Name or service not known" in error_msg:
                key = "DNS解析失败"
            else:
                key = f"连接器错误: {error_msg[:50]}"
        elif "ClientOSError" in error_type:
            key = f"系统错误: {error_msg[:50]}"
        else:
            key = f"{error_type}: {error_msg[:50]}"
        
        self.errors[key] += 1
    
    def print_error_summary(self):
        """打印错误统计"""
        if not self.errors:
            print("没有错误记录")
            return
        
        print("\n错误类型统计:")
        print("-" * 50)
        # 按数量排序
        sorted_errors = sorted(self.errors.items(), key=lambda x: x[1], reverse=True)
        for error_type, count in sorted_errors:
            pct = count / self.failed * 100 if self.failed > 0 else 0
            print(f"  {error_type}: {count} ({pct:.1f}%)")
        print("-" * 50)


stats = Stats()
running = True


def generate_channel_id(prefix: str = "stress-test") -> str:
    """生成随机 channel_id"""
    suffix = ''.join(random.choices(string.ascii_lowercase + string.digits, k=8))
    return f"{prefix}-{suffix}"


async def create_sse_connection(
    session: aiohttp.ClientSession,
    url: str,
    channel_id: str,
    connection_id: int,
    duration: int,
    verbose: bool = False
):
    """创建单个 SSE 连接"""
    global stats
    
    sse_url = f"{url}/sse/connect?channel_id={channel_id}"
    
    try:
        async with session.get(sse_url, timeout=aiohttp.ClientTimeout(total=None)) as response:
            if response.status == 200:
                stats.connected += 1
                if verbose:
                    print(f"[{connection_id}] 连接成功: {channel_id}")
                
                # 读取 SSE 事件
                start = time.time()
                async for line in response.content:
                    if not running:
                        break
                    
                    # 检查是否超过持续时间
                    if duration > 0 and (time.time() - start) > duration:
                        break
                    
                    line = line.decode('utf-8', errors='ignore').strip()
                    if line.startswith('data:'):
                        stats.messages_received += 1
                        if verbose:
                            print(f"[{connection_id}] 收到消息")
                
                stats.disconnected += 1
            else:
                stats.failed += 1
                # 记录 HTTP 错误
                stats.errors[f"HTTP {response.status}"] += 1
                if verbose:
                    print(f"[{connection_id}] 连接失败: HTTP {response.status}")
                    
    except asyncio.CancelledError:
        stats.disconnected += 1
    except Exception as e:
        stats.failed += 1
        stats.add_error(e)
        if verbose:
            print(f"[{connection_id}] 错误: {e}")


async def create_connections_batch(
    url: str,
    count: int,
    batch_size: int,
    channel_mode: str,
    channel_prefix: str,
    duration: int,
    verbose: bool,
    skip_ssl: bool = False
):
    """分批创建连接"""
    global stats
    stats.total = count
    stats.start_time = time.time()
    
    # 创建 SSL 上下文
    ssl_context = None
    if skip_ssl:
        ssl_context = ssl.create_default_context()
        ssl_context.check_hostname = False
        ssl_context.verify_mode = ssl.CERT_NONE
    
    # 创建连接器，设置较大的连接池
    connector = aiohttp.TCPConnector(
        limit=0,  # 无限制
        limit_per_host=0,
        force_close=False,
        enable_cleanup_closed=True,
        ssl=ssl_context
    )
    
    timeout = aiohttp.ClientTimeout(connect=30)
    
    async with aiohttp.ClientSession(connector=connector, timeout=timeout) as session:
        tasks = []
        
        for i in range(count):
            if not running:
                break
            
            # 根据模式生成 channel_id
            if channel_mode == "random":
                channel_id = generate_channel_id(channel_prefix)
            elif channel_mode == "shared":
                channel_id = channel_prefix
            else:  # sequential
                channel_id = f"{channel_prefix}-{i}"
            
            task = asyncio.create_task(
                create_sse_connection(
                    session, url, channel_id, i, duration, verbose
                )
            )
            tasks.append(task)
            
            # 分批创建，避免一次性发起太多连接
            if (i + 1) % batch_size == 0:
                print(f"\r创建进度: {i + 1}/{count} - {stats}", end="", flush=True)
                await asyncio.sleep(0.01)  # 短暂暂停
        
        print(f"\n\n所有 {count} 个连接请求已发出，等待连接建立...")
        print("按 Ctrl+C 停止测试\n")
        
        # 等待所有连接完成或被取消
        try:
            while running and stats.connected + stats.failed < count:
                print(f"\r{stats}", end="", flush=True)
                await asyncio.sleep(1)
            
            print(f"\n\n所有连接已建立，保持 {duration if duration > 0 else '无限'} 秒...")
            
            # 持续显示统计
            while running:
                print(f"\r{stats}", end="", flush=True)
                await asyncio.sleep(1)
                
        except asyncio.CancelledError:
            pass
        
        # 取消所有任务
        print("\n\n正在关闭连接...")
        for task in tasks:
            task.cancel()
        
        await asyncio.gather(*tasks, return_exceptions=True)


def signal_handler(sig, frame):
    """处理 Ctrl+C"""
    global running
    print("\n\n收到停止信号...")
    running = False


def main():
    parser = argparse.ArgumentParser(
        description="SSE Gateway 压力测试 - 批量创建 SSE 连接",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
示例:
  python stress_test.py                              # 默认1万连接到线上服务
  python stress_test.py --count 5000                 # 5000个连接
  python stress_test.py --url http://localhost:8080  # 测试本地服务
  python stress_test.py --channel-mode shared --channel-prefix my-channel  # 所有连接使用同一个channel
  python stress_test.py --duration 60                # 保持连接60秒后自动断开
  python stress_test.py --batch 500                  # 每批创建500个连接
        """
    )
    
    parser.add_argument(
        "--url", "-u",
        default="https://gateway-sse-912773111852.asia-east1.run.app",
        help="SSE Gateway 服务 URL (默认: 线上服务)"
    )
    
    parser.add_argument(
        "--count", "-c",
        type=int,
        default=10000,
        help="创建的连接数量 (默认: 10000)"
    )
    
    parser.add_argument(
        "--batch", "-b",
        type=int,
        default=200,
        help="每批创建的连接数 (默认: 200)"
    )
    
    parser.add_argument(
        "--channel-mode", "-m",
        choices=["random", "shared", "sequential"],
        default="random",
        help="Channel ID 模式: random=随机, shared=共享同一个, sequential=顺序编号 (默认: random)"
    )
    
    parser.add_argument(
        "--channel-prefix", "-p",
        default="stress-test",
        help="Channel ID 前缀 (默认: stress-test)"
    )
    
    parser.add_argument(
        "--duration", "-d",
        type=int,
        default=0,
        help="保持连接的时间(秒), 0表示无限 (默认: 0)"
    )
    
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="显示详细日志"
    )
    
    parser.add_argument(
        "--skip-ssl", "-k",
        action="store_true",
        help="跳过 SSL 证书验证（解决 macOS Python SSL 问题）"
    )
    
    args = parser.parse_args()
    
    # 设置信号处理
    signal.signal(signal.SIGINT, signal_handler)
    signal.signal(signal.SIGTERM, signal_handler)
    
    print("=" * 60)
    print("SSE Gateway 压力测试")
    print("=" * 60)
    print(f"目标服务: {args.url}")
    print(f"连接数量: {args.count}")
    print(f"批次大小: {args.batch}")
    print(f"Channel 模式: {args.channel_mode}")
    print(f"Channel 前缀: {args.channel_prefix}")
    print(f"持续时间: {args.duration if args.duration > 0 else '无限'}秒")
    print(f"跳过SSL验证: {'是' if args.skip_ssl else '否'}")
    print("=" * 60)
    print()
    
    try:
        asyncio.run(
            create_connections_batch(
                url=args.url,
                count=args.count,
                batch_size=args.batch,
                channel_mode=args.channel_mode,
                channel_prefix=args.channel_prefix,
                duration=args.duration,
                verbose=args.verbose,
                skip_ssl=args.skip_ssl
            )
        )
    except KeyboardInterrupt:
        pass
    
    print("\n" + "=" * 60)
    print("测试结束")
    print("=" * 60)
    print(f"最终统计: {stats}")
    
    # 打印错误统计
    if stats.failed > 0:
        stats.print_error_summary()
    
    # 计算成功率
    success_rate = stats.connected / stats.total * 100 if stats.total > 0 else 0
    print(f"\n成功率: {success_rate:.1f}%")
    print("=" * 60)


if __name__ == "__main__":
    main()
