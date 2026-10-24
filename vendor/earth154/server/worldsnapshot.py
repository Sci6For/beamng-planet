"""Session-scoped, process-safe first-use pins for LAN physical source data."""
import hashlib
import json
import os
import re
import sys
from contextlib import contextmanager
from contextvars import ContextVar
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path

from worldmanifest import InputDependency, WorldManifest

_job_dependencies = ContextVar("planet_world_dependencies", default=None)


@lru_cache(maxsize=1)
def generator_fingerprints():
    """Identify the exact Python/native physical producer loaded in this process."""
    source_dir = Path(__file__).parent
    modules = sorted(p.name for p in source_dir.glob("*.py"))
    source = {name: hashlib.sha256((source_dir / name).read_bytes()).hexdigest() for name in modules}
    import fast
    native = getattr(fast, "_ef", None)
    native_file = getattr(native, "__file__", None)
    return {"python": sys.version.split()[0], "modules": source,
            "nativeVersion": getattr(native, "VERSION", None),
            "nativeSha256": hashlib.sha256(Path(native_file).read_bytes()).hexdigest() if native_file else None}


class WorldInputUnavailable(IOError):
    """A physical input cannot be pinned without changing the admitted world."""


@dataclass(frozen=True)
class PhysicsFrame:
    frame_id: str
    epoch: int
    latitude: float
    longitude: float
    height: float
    quantum: float

    def __post_init__(self):
        if self.quantum <= 0 or self.quantum > 1 or self.quantum.as_integer_ratio()[1] & (
                self.quantum.as_integer_ratio()[1] - 1):
            raise ValueError("frame quantum must be a positive binary fraction <= 1 m")


class WorldSnapshot:
    """The first successful pin per (room, revision, kind, key) wins across processes."""

    def __init__(self, room_id, cache_root, config):
        self.room_id = str(room_id)
        if re.fullmatch(r"[0-9a-f]{32}", self.room_id) is None:
            raise ValueError("room ID must be 32 lowercase hex characters")
        self.config = dict(config)
        self.config.setdefault("generator", generator_fingerprints())
        self.world_revision = str(self.config.get("revision", "1"))
        if re.fullmatch(r"[0-9]+", self.world_revision) is None:
            raise ValueError("world revision must be an unsigned decimal string")
        encoded = json.dumps(self.config, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
        self.config_sha256 = hashlib.sha256(encoded).hexdigest()
        self.root = Path(cache_root) / "planet-world" / self.room_id / self.world_revision
        self.blobs = self.root / "blobs"
        self.keys = self.root / "keys"
        self.blobs.mkdir(parents=True, exist_ok=True)
        self.keys.mkdir(parents=True, exist_ok=True)
        config_path = self.root / "config.json"
        self._publish_bytes(config_path, encoded)
        if config_path.read_bytes() != encoded:
            raise ValueError("room revision already has a different physical configuration")

    @classmethod
    def open(cls, room_id, cache_root, config):
        return cls(room_id, cache_root, config)

    def _key_path(self, kind, logical_key):
        if not kind or not logical_key:
            raise ValueError("physical input kind and logical key are required")
        key = json.dumps([kind, logical_key], separators=(",", ":")).encode()
        return self.keys / (hashlib.sha256(key).hexdigest() + ".json")

    @staticmethod
    def _publish_bytes(path, data):
        """Publish a complete same-volume inode once; another process never sees staging bytes."""
        if path.exists():
            return
        tmp = path.parent / f".{path.name}.{os.getpid()}.{os.urandom(8).hex()}.tmp"
        try:
            with tmp.open("xb") as f:
                f.write(data)
                f.flush()
                os.fsync(f.fileno())
            try:
                os.link(tmp, path)
            except FileExistsError:
                pass
        finally:
            tmp.unlink(missing_ok=True)

    def pin_input(self, kind, logical_key, data):
        data = bytes(data)
        sha = hashlib.sha256(data).hexdigest()
        path = self.blobs / sha
        self._publish_bytes(path, data)
        record = {"kind": kind, "logicalKey": logical_key, "sha256": sha, "byteLength": len(data)}
        key_path = self._key_path(kind, logical_key)
        tmp = key_path.parent / f".{key_path.name}.{os.getpid()}.{os.urandom(8).hex()}.tmp"
        try:
            with tmp.open("x", encoding="utf-8") as f:
                json.dump(record, f, sort_keys=True)
                f.flush()
                os.fsync(f.fileno())
            try:
                os.link(tmp, key_path)
            except FileExistsError:
                pass
        finally:
            tmp.unlink(missing_ok=True)
        return self._record(kind, logical_key)["sha256"]

    def _record(self, kind, logical_key):
        path = self._key_path(kind, logical_key)
        try:
            record = json.loads(path.read_text(encoding="utf-8"))
        except FileNotFoundError as e:
            raise WorldInputUnavailable(f"unadmitted physical input: {kind}/{logical_key}") from e
        if record["kind"] != kind or record["logicalKey"] != logical_key:
            raise WorldInputUnavailable("physical input key collision")
        return record

    def resolve_input(self, kind, logical_key):
        record = self._record(kind, logical_key)
        path = self.blobs / record["sha256"]
        try:
            data = path.read_bytes()
        except FileNotFoundError as e:
            raise WorldInputUnavailable(f"pinned physical input disappeared: {kind}/{logical_key}") from e
        if len(data) != record["byteLength"] or hashlib.sha256(data).hexdigest() != record["sha256"]:
            raise WorldInputUnavailable(f"pinned physical input corrupt: {kind}/{logical_key}")
        used = _job_dependencies.get()
        if used is not None:
            used.add((kind, logical_key))
        return path

    @contextmanager
    def job(self):
        used = set()
        token = _job_dependencies.set(used)
        try:
            yield used
        finally:
            _job_dependencies.reset(token)

    def has_input(self, kind, logical_key):
        return self._key_path(kind, logical_key).exists()

    def note_input(self, kind, logical_key):
        used = _job_dependencies.get()
        if used is not None:
            used.add((kind, logical_key))

    def pinned_inputs(self):
        """All physical inputs admitted so far, including pins made by worker processes."""
        keys = []
        for path in self.keys.glob("*.json"):
            try:
                r = json.loads(path.read_text(encoding="utf-8"))
                keys.append((r["kind"], r["logicalKey"]))
            except (OSError, ValueError, KeyError):
                raise WorldInputUnavailable(f"incomplete physical input index: {path}") from None
        return tuple(sorted(set(keys)))

    def seal_job(self, dependencies, physics_complete):
        if not physics_complete:
            raise WorldInputUnavailable("incomplete physical inputs cannot seal a LAN artifact")
        entries = []
        for kind, logical_key in dependencies:
            self.resolve_input(kind, logical_key)
            r = self._record(kind, logical_key)
            entries.append(InputDependency(kind, logical_key, r["sha256"], r["byteLength"]))
        return WorldManifest(self.room_id, self.world_revision, self.config_sha256,
                             tuple(sorted(entries, key=lambda e: (e.kind, e.logical_key))), True)
