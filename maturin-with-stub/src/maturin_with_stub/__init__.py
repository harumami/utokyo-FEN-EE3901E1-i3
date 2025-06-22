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
    path = pathlib.Path(
        json.loads(
            run_command(
                ["uv", "python", "list", "--only-installed", "--output-format", "json"]
            )
        )[0]["path"]
    ).parent

    command = ["cargo", "run", "--bin", "stub_gen"]

    if release:
        command.append("--release")

    run_command(command, path=path)


def run_command(command: list[str], path: str | None = None) -> bytes:
    env = os.environ.copy()

    if path is not None:
        env["PATH"] = f"{path}{os.pathsep}{env['PATH']}"

    print(f"Running `{' '.join(command)}`")
    sys.stdout.flush()
    result = subprocess.run(command, stdout=subprocess.PIPE, env=env, check=False)  # noqa: S603
    sys.stdout.buffer.write(result.stdout)
    sys.stdout.flush()

    if result.returncode != 0:
        sys.stderr.write(
            f"Error: command `{' '.join(command)}` returned non-zero exit status {result.returncode}\n"  # noqa: E501
        )

        sys.exit(1)

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
