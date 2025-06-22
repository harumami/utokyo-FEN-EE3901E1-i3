$env.PATH = [(uv python list --only-installed --output-format json | from json | $in.0.path | path parse | $in.parent)] ++ $env.PATH
cargo run -p phone-py --bin stub_gen
