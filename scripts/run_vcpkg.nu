def main [--triplet: string] {
  let triplet = match $triplet {
    null => {
      let arch = match $nu.os-info.arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        $arch => (error make -u { msg: $'unknown arch: ($arch)' }),
      };

      let os = match $nu.os-info.family {
        "windows" => "windows-static-md",
        "macos" => "osx",
        "linux" => "linux",
        $os => (error make -u { msg: $'unknown os: ($os)' })
      };

      $'($arch)-($os)'
    }
    _ => $triplet
  }

  vcpkg install --triplet $triplet
  mkdir packages/opus-sys/vcpkg
}
