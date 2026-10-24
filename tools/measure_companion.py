"""Read-only Windows process sampler. Never signals, stops, or restarts a process.

The operator must keep the actual game in its menu throughout the 300-second
measurement and record that observation separately. CPU is on a one-core basis.
"""
import argparse
from datetime import datetime, timezone
import json
import math
from pathlib import Path
import sys
import time

import psutil


def positive_number(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError('must be a finite positive number')
    return number


def measure(pid, duration, interval, expected_create_time=None):
    process = psutil.Process(pid)
    created = process.create_time()
    if expected_create_time is not None and created != expected_create_time:
        raise ValueError('PID creation time differs from the expected process')
    image = process.exe()
    started_utc = datetime.now(timezone.utc).isoformat()
    start = time.monotonic()
    samples = []
    termination = 'duration_reached'
    while True:
        try:
            # A new Process avoids cached create_time values when detecting PID reuse.
            current = psutil.Process(pid)
            if current.create_time() != created:
                termination = 'identity_changed'
                break
            cpu = current.cpu_times()
            private = current.memory_full_info().private
            if psutil.Process(pid).create_time() != created:
                termination = 'identity_changed'
                break
            elapsed = time.monotonic() - start
            samples.append(dict(elapsedSeconds=elapsed, cpuSeconds=cpu.user + cpu.system, privateBytes=private))
        except psutil.NoSuchProcess:
            termination = 'process_exited'
            break
        except psutil.AccessDenied:
            termination = 'access_denied'
            break
        except KeyboardInterrupt:
            termination = 'interrupted'
            break
        remaining = duration - (time.monotonic() - start)
        if remaining <= 0:
            break
        try:
            time.sleep(min(interval, remaining))
        except KeyboardInterrupt:
            termination = 'interrupted'
            break
    wall = samples[-1]['elapsedSeconds'] - samples[0]['elapsedSeconds'] if len(samples) >= 2 else 0
    cpu_delta = samples[-1]['cpuSeconds'] - samples[0]['cpuSeconds'] if len(samples) >= 2 else 0
    private_values = [sample['privateBytes'] for sample in samples]
    return dict(formatVersion=1, pid=pid, createTime=created, executable=image, startedAtUtc=started_utc,
                requestedDurationSeconds=duration, intervalSeconds=interval, elapsedWallSeconds=time.monotonic() - start,
                observedWallSeconds=wall, cpuSeconds=cpu_delta, cpuPercentOneCore=cpu_delta / wall * 100 if wall else None,
                privatePeakMiB=max(private_values) / 1048576 if private_values else None,
                privateMeanMiB=sum(private_values) / len(private_values) / 1048576 if private_values else None,
                termination=termination, samples=samples)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', type=int, required=True)
    parser.add_argument('--expected-create-time', type=float)
    parser.add_argument('--duration', type=positive_number, default=300)
    parser.add_argument('--interval', type=positive_number, default=1)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.output.exists():
            raise ValueError('output already exists; choose a new filename')
        if not args.output.is_absolute() or not args.output.parent.is_dir():
            raise ValueError('output must be absolute with an existing parent directory')
        report = measure(args.pid, args.duration, args.interval, args.expected_create_time)
        with args.output.open('x', encoding='utf-8') as output:
            json.dump(report, output, indent=2, allow_nan=False)
            output.write('\n')
        print(json.dumps({key: value for key, value in report.items() if key != 'samples'}, indent=2))
    except (ValueError, OSError, psutil.Error, AttributeError) as error:
        print(f'Measurement failed: {error}', file=sys.stderr)
        return 2
    return 0


if __name__ == '__main__':
    sys.exit(main())
