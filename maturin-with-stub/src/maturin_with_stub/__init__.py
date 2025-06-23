import contextlib
import json
import os
import pathlib
import subprocess
import sys

import maturin
from maturin import (
    build_sdist,
    get_requires_for_build_editable,
    get_requires_for_build_sdist,
    get_requires_for_build_wheel,
    prepare_metadata_for_build_editable,
    prepare_metadata_for_build_wheel,
)


def build_wheel(
    wheel_directory: str,
    config_settings: dict[str, object] | None = None,
    metadata_directory: str | None = None,
) -> str:
    stub_gen(release=True)
    return maturin.build_wheel(wheel_directory, config_settings, metadata_directory)


def build_editable(
    wheel_directory: str,
    config_settings: dict[str, object] | None = None,
    metadata_directory: str | None = None,
) -> str:
    stub_gen(release=False)
    return maturin.build_editable(wheel_directory, config_settings, metadata_directory)


def stub_gen(*, release: bool) -> None:
    env = None
    with contextlib.suppress(subprocess.CalledProcessError):
        paths = []

        for instance in json.loads(
            run_command(
                ["uv", "python", "list", "--only-installed", "--output-format", "json"]
            )
        ):
            path = pathlib.PurePath(instance["path"])
            paths.append(path.parent)
            paths.append(path.parent.parent / "lib")

        env = os.environ.copy()
        paths.append(env["PATH"])
        env["PATH"] = os.pathsep.join(map(str, paths))

    command = ["cargo", "run", "--bin", "stub_gen"]

    if release:
        command.append("--release")

    run_command(command, env=env)


def run_command(
    command: list[str],
    *,
    env: dict[str, str] | None = None,
    check: bool=True
) -> bytes:
    print(f"Running `{' '.join(command)}`")
    sys.stdout.flush()
    result = subprocess.run(command, stdout=subprocess.PIPE, env=env, check=check)  # noqa: S603
    sys.stdout.buffer.write(result.stdout)
    sys.stdout.flush()
    return result.stdout


__all__ = [
    "build_editable",
    "build_sdist",
    "build_wheel",
    "get_requires_for_build_editable",
    "get_requires_for_build_sdist",
    "get_requires_for_build_wheel",
    "prepare_metadata_for_build_editable",
    "prepare_metadata_for_build_wheel",
]
