ARG BASE_IMAGE=ubuntu:24.04
FROM ${BASE_IMAGE}

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl git sudo build-essential pkg-config \
      clang libclang-dev libelf-dev zlib1g-dev autoconf automake libtool autopoint bison flex gawk \
      libcap2-bin linux-tools-generic linux-tools-common \
    && rm -rf /var/lib/apt/lists/*

# Non-root with passwordless sudo, so the setcap path is exercised like on a dev machine.
RUN userdel -r ubuntu && useradd -m -u 1000 -s /bin/bash tester \
    && echo 'tester ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/tester
USER tester
ENV PATH=/home/tester/.cargo/bin:$PATH \
    CARGO_TARGET_DIR=/home/tester/target \
    CODSPEED_ISOLATION=false

ARG RUST_VERSION
ARG RUST_COMPONENTS
# Components from rust-toolchain.toml, else rustup installs them on every `--rm` run.
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "$RUST_VERSION" \
      --component "$RUST_COMPONENTS" \
    && mkdir -p /home/tester/target /home/tester/.cargo/registry /home/tester/.cargo/git

# Starts as root only to hand `tester` an ambient CAP_PERFMON: samply runs unprivileged
# without systemd-run, and the host's perf_event_paranoid is not namespaced.
USER root
ENTRYPOINT ["setpriv", "--reuid=tester", "--regid=tester", "--init-groups", \
            "--inh-caps=+perfmon", "--ambient-caps=+perfmon", "env", "HOME=/home/tester"]
WORKDIR /workspace
