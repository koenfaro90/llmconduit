#!/bin/sh
set -eu

# The source checkout is private to the host user; the durable database belongs
# to the production image's nonroot uid. Keep those permissions intact while
# cargo-watch supervises the compile-and-run cycle.
source_uid=$(stat -c %u /app/Cargo.toml)
source_gid=$(stat -c %g /app/Cargo.toml)
exec cargo watch \
  -w src \
  -w migrations \
  -w Cargo.toml \
  -w Cargo.lock \
  -w build.rs \
  -s "setpriv --reuid=$source_uid --regid=$source_gid --clear-groups env CARGO_HOME=/cargo-home CARGO_TARGET_DIR=/cargo-target CARGO_PROFILE_DEV_DEBUG=0 cargo build --locked && setpriv --reuid=65532 --regid=65532 --clear-groups /cargo-target/debug/llmconduit start --config /etc/llmconduit/config.yaml --with-debug-ui"
