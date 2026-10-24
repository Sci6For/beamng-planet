"""Content identities for physical world inputs and generated jobs."""
from dataclasses import dataclass


@dataclass(frozen=True)
class InputDependency:
    kind: str
    logical_key: str
    sha256: str
    byte_length: int


@dataclass(frozen=True)
class WorldManifest:
    room_id: str
    world_revision: str
    config_sha256: str
    dependencies: tuple[InputDependency, ...]
    physics_complete: bool

