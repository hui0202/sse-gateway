#!/usr/bin/env python3
import asyncio
import aiohttp
import time
import os
import sys
import statistics
import json
from dataclasses import dataclass
from typing import List, Optional
from datetime import datetime

@dataclass
class Config:
    host: str = "http://localhost:8080"
    concurrent_connections: int = 50
    push_count: int = 200
    push_concurrency: int = 20
    e2e_messages: int = 20
    
    @classmethod
    def from_env(cls, host: Optional[str] = None):
        return cls(
            host=host or os.getenv("HOST", "http://localhost:8080"),
            concurrent_connections=int(os.getenv("CONCURRENT_CONNECTIONS", "50")),
            push_count=int(os.getenv("PUSH_COUNT", "200")),
            push_concurrency=int(os.getenv("PUSH_CONCURRENCY", "20")),
            e2e_messages=int(os.getenv("E2E_MESSAGES", "20")),
        )

class Colors:
    CYAN = '\033[0;36m'
    GREEN = '\033[0;32m'
    YELLOW = '\033[1;33m'
    RED = '\033[0;31m'
    BOLD = '\033[1m'
    NC = '\033[0m'

def now_ms() -> int:
    return int(time.time() * 1000)

def print_header(title: str):
    print(f"\n{Colors.CYAN}╔══════════════════════════════════════════════════════════════╗{Colors.NC}")
    print(f"{Colors.CYAN}║{Colors.NC} {Colors.BOLD}{title}{Colors.NC}")
    print(f"{Colors.CYAN}╚══════════════════════════════════════════════════════════════╝{Colors.NC}")

def print_ok(msg: str):
    print(f"  {Colors.GREEN}✓{Colors.NC} {msg}")

def print_warn(msg: str):
    print(f"  {Colors.YELLOW}⚠{Colors.NC} {msg}")

def print_error(msg: str):
    print(f"  {Colors.RED}✗{Colors.NC} {msg}")

def print_stat(label: str, value: str):
    print(f"  {label:<20} {value}")

def percentile(data: List[float], p: float) -> float:
    if not data:
        return 0.0
    sorted_data = sorted(data)
    idx = int(len(sorted_data) * p / 100)
    idx = min(idx, len(sorted_data) - 1)
    return sorted_data[idx]

def print_latency_stats(label: str, latencies: List[float]):
    if not latencies:
        print_error(f"{label}: no data")
        return
    
    print(f"  {Colors.BOLD}{label}{Colors.NC} (sample count: {len(latencies)})")
    print_stat("Min:", f"{min(latencies):.1f}ms")
    print_stat("Avg:", f"{statistics.mean(latencies):.1f}ms")
    print_stat("P50:", f"{percentile(latencies, 50):.1f}ms")
    print_stat("P95:", f"{percentile(latencies, 95):.1f}ms")
    print_stat("P99:", f"{percentile(latencies, 99):.1f}ms")
    print_stat("Max:", f"{max(latencies):.1f}ms")

async def test_health(config: Config):
    print_header("Test 1: Health Check")
    
    start = now_ms()
    async with aiohttp.ClientSession() as session:
        try:
            async with session.get(f"{config.host}/health", timeout=aiohttp.ClientTimeout(total=5)) as resp:
                latency = now_ms() - start
                if resp.status == 200:
                    print_ok(f"Health check passed (HTTP {resp.status}, latency: {latency}ms)")
                else:
                    print_error(f"Health check failed (HTTP {resp.status})")
                    return False
        except Exception as e:
            print_error(f"Health check failed: {e}")
            return False
        
        try:
            async with session.get(f"{config.host}/api/stats", timeout=aiohttp.ClientTimeout(total=5)) as resp:
                if resp.status == 200:
                    stats = await resp.json()
                    print_ok(f"Current connection count: {stats.get('total_connections', 'N/A')}")
        except:
            pass
    
    return True

async def measure_sse_ttfb(session: aiohttp.ClientSession, url: str, timeout: float = 5.0) -> Optional[float]:
    start = now_ms()
    try:
        async with session.get(url, timeout=aiohttp.ClientTimeout(
            total=timeout,
            connect=5.0,
            sock_read=timeout
        )) as resp:
            async for _ in resp.content.iter_any():
                return now_ms() - start
    except asyncio.TimeoutError:
        return None
    except Exception:
        return None
    return None

async def test_sse_connection_latency(config: Config):
    print_header(f"Test 2: SSE connection establishment latency ({config.concurrent_connections} connections)")
    
    connector = aiohttp.TCPConnector(limit=0, force_close=True)
    async with aiohttp.ClientSession(connector=connector) as session:
        
        print(f"  {Colors.BOLD}Single connection latency (10 samples):{Colors.NC}")
        single_latencies = []
        for i in range(10):
            url = f"{config.host}/sse/connect?channel_id=latency_test_{i}"
            latency = await measure_sse_ttfb(session, url, timeout=2.0)
            if latency:
                single_latencies.append(latency)
        
        print_latency_stats("SSE single connection TTFB", single_latencies)
        
        print(f"\n  {Colors.BOLD}Concurrent connection test ({config.concurrent_connections} concurrent connections):{Colors.NC}")
        
        async def single_connect(i: int) -> Optional[float]:
            url = f"{config.host}/sse/connect?channel_id=concurrent_test_{i}"
            return await measure_sse_ttfb(session, url, timeout=5.0)
        
        start = now_ms()
        tasks = [single_connect(i) for i in range(config.concurrent_connections)]
        results = await asyncio.gather(*tasks)
        total_time = now_ms() - start
        
        concurrent_latencies = [r for r in results if r is not None]
        errors = len(results) - len(concurrent_latencies)
        
        print_latency_stats("Concurrent SSE connection TTFB", concurrent_latencies)
        print_stat("Total time:", f"{total_time}ms")
        print_stat("Success/Failure:", f"{len(concurrent_latencies)}/{errors}")
        if total_time > 0:
            print_stat("Throughput:", f"{len(concurrent_latencies) * 1000 / total_time:.2f} conn/s")

async def test_push_throughput(config: Config):
    print_header(f"Test 3: Message push throughput ({config.push_count} messages, {config.push_concurrency} concurrent)")
    
    latencies = []
    errors = 0
    semaphore = asyncio.Semaphore(config.push_concurrency)
    
    async def single_push(session: aiohttp.ClientSession, i: int) -> Optional[float]:
        nonlocal errors
        async with semaphore:
            payload = {
                "channel_id": "throughput_test",
                "event_type": "benchmark",
                "data": {"seq": i, "ts": now_ms()}
            }
            start = now_ms()
            try:
                async with session.post(
                    f"{config.host}/api/send",
                    json=payload,
                    timeout=aiohttp.ClientTimeout(total=10)
                ) as resp:
                    latency = now_ms() - start
                    if resp.status == 200:
                        return latency
                    else:
                        errors += 1
                        return None
            except Exception:
                errors += 1
                return None
    
    connector = aiohttp.TCPConnector(limit=0)
    async with aiohttp.ClientSession(connector=connector) as session:
        start = now_ms()
        tasks = [single_push(session, i) for i in range(config.push_count)]
        results = await asyncio.gather(*tasks)
        total_time = now_ms() - start
    
    latencies = [r for r in results if r is not None]

    print_latency_stats("Push request latency", latencies)
    print()
    print_stat("Total time:", f"{total_time}ms")
    print_stat("Success/Failure:", f"{len(latencies)}/{errors}")
    if total_time > 0:
        print_stat("Throughput:", f"{len(latencies) * 1000 / total_time:.2f} msg/s")

async def test_e2e_latency(config: Config):
    print_header(f"Test 4: End-to-end latency (Publish -> SSE Receive) ({config.e2e_messages} messages)")
    
    channel_id = f"e2e_bench_{os.getpid()}_{int(time.time())}"
    latencies = []
    errors = 0
    received_markers = {}  # marker -> receive_time
    
    async def sse_receiver(session: aiohttp.ClientSession, stop_event: asyncio.Event):
        url = f"{config.host}/sse/connect?channel_id={channel_id}"
        try:
            async with session.get(url, timeout=aiohttp.ClientTimeout(total=None)) as resp:
                buffer = ""
                async for chunk in resp.content.iter_any():
                    if stop_event.is_set():
                        break
                    buffer += chunk.decode('utf-8', errors='ignore')
                    
                    while "\n\n" in buffer:
                        event_str, buffer = buffer.split("\n\n", 1)
                        recv_time = now_ms()
                        
                        for line in event_str.split("\n"):
                            if line.startswith("data:"):
                                try:
                                    data = json.loads(line[5:].strip())
                                    marker = data.get("marker")
                                    if marker and marker.startswith("e2e_"):
                                        received_markers[marker] = recv_time
                                except:
                                    pass
        except asyncio.CancelledError:
            pass
        except Exception as e:
            print_warn(f"SSE receiver exception: {e}")
    
    connector = aiohttp.TCPConnector(limit=0)
    async with aiohttp.ClientSession(connector=connector) as session:
        print_ok(f"Establishing SSE connection (channel: {channel_id})...")
        
        stop_event = asyncio.Event()
        receiver_task = asyncio.create_task(sse_receiver(session, stop_event))
        
        await asyncio.sleep(1.0)
        print_ok("SSE connection established")
        
        for i in range(config.e2e_messages):
            marker = f"e2e_{i}_{time.time_ns()}"
            send_ts = now_ms()
            
            payload = {
                "channel_id": channel_id,
                "event_type": "e2e_test",
                "data": {"marker": marker, "send_ts": send_ts}
            }
            
            try:
                async with session.post(f"{config.host}/api/send", json=payload, 
                                       timeout=aiohttp.ClientTimeout(total=5)) as resp:
                    if resp.status != 200:
                        errors += 1
                        continue
            except Exception:
                errors += 1
                continue
            
            for _ in range(50):
                if marker in received_markers:
                    e2e_latency = received_markers[marker] - send_ts
                    latencies.append(e2e_latency)
                    break
                await asyncio.sleep(0.1)
            else:
                errors += 1
                print_warn(f"Message #{i} timeout")
        
        stop_event.set()
        receiver_task.cancel()
        try:
            await receiver_task
        except asyncio.CancelledError:
            pass
    
    print()
    print_latency_stats("End-to-end latency (Publish -> SSE Receive)", latencies)
    print()
    print_stat("Success/Failure:", f"{len(latencies)}/{errors}")
    if latencies:
        print_ok("This is the full latency from sending HTTP POST to SSE client")

async def test_concurrent_broadcast(config: Config):
    num_clients = 10
    print_header(f"Test 5: Concurrent broadcast test ({num_clients} SSE clients simultaneously receiving)")
    
    channel_id = f"broadcast_bench_{os.getpid()}"
    client_received = {}  # client_id -> receive_time
    
    async def sse_client(session: aiohttp.ClientSession, client_id: int, 
                         marker: str, stop_event: asyncio.Event):
        url = f"{config.host}/sse/connect?channel_id={channel_id}"
        try:
            async with session.get(url, timeout=aiohttp.ClientTimeout(total=None)) as resp:
                buffer = ""
                async for chunk in resp.content.iter_any():
                    if stop_event.is_set():
                        break
                    buffer += chunk.decode('utf-8', errors='ignore')
                    
                    if marker in buffer and client_id not in client_received:
                        client_received[client_id] = now_ms()
                        break
        except asyncio.CancelledError:
            pass
        except:
            pass
    
    connector = aiohttp.TCPConnector(limit=0)
    async with aiohttp.ClientSession(connector=connector) as session:
        print_ok(f"Starting {num_clients} SSE clients...")
        
        marker = f"bcast_{time.time_ns()}"
        stop_event = asyncio.Event()
        
        client_tasks = [
            asyncio.create_task(sse_client(session, i, marker, stop_event))
            for i in range(num_clients)
        ]
        
        await asyncio.sleep(1.5)
        print_ok(f"Clients connected")
        
        send_ts = now_ms()
        payload = {
            "channel_id": channel_id,
            "event_type": "broadcast",
            "data": {"marker": marker, "send_ts": send_ts}
        }
        
        await session.post(f"{config.host}/api/send", json=payload)
        print_ok("Message sent, waiting for all clients to receive...")
        
        for _ in range(50):
            if len(client_received) >= num_clients:
                break
            await asyncio.sleep(0.1)
        
        final_ts = now_ms()
        broadcast_latency = final_ts - send_ts
        
        stop_event.set()
        for task in client_tasks:
            task.cancel()
        await asyncio.gather(*client_tasks, return_exceptions=True)

        print_stat("Received clients:", f"{len(client_received)}/{num_clients}")
        print_stat("Broadcast latency (all received):", f"{broadcast_latency}ms")
        
        if client_received:
            individual_latencies = [t - send_ts for t in client_received.values()]
            print_stat("Individual client latency range:", f"{min(individual_latencies)}-{max(individual_latencies)}ms")

async def main():
    host = sys.argv[1] if len(sys.argv) > 1 else None
    config = Config.from_env(host)
    
    print(f"{Colors.CYAN}╔══════════════════════════════════════════════════════════════╗{Colors.NC}")
    print(f"{Colors.CYAN}║{Colors.NC}       {Colors.BOLD}SSE Gateway remote performance test (Python){Colors.NC}")
    print(f"{Colors.CYAN}╚══════════════════════════════════════════════════════════════╝{Colors.NC}")
    print()
    print(f"  {Colors.BOLD}Target address:{Colors.NC}        {config.host}")
    print(f"  {Colors.BOLD}Concurrent connections:{Colors.NC}      {config.concurrent_connections}")
    print(f"  {Colors.BOLD}Push messages:{Colors.NC}      {config.push_count}")
    print(f"  {Colors.BOLD}Push concurrency:{Colors.NC}      {config.push_concurrency}")
    print(f"  {Colors.BOLD}E2E messages:{Colors.NC}      {config.e2e_messages}")
    
    # Health check
    if not await test_health(config):
        print_error("Service unavailable, exiting test")
        return
    
    # Run tests
    await test_sse_connection_latency(config)
    await test_push_throughput(config)
    await test_e2e_latency(config)
    await test_concurrent_broadcast(config)
        
    print_header("测试完成")
    print()
    print("  Parameters can be adjusted via environment variables:")
    print(f"    CONCURRENT_CONNECTIONS=100 PUSH_COUNT=500 python {sys.argv[0]} {config.host}")
    print()

if __name__ == "__main__":
    asyncio.run(main())
