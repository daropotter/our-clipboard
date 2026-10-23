FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends \
    xvfb xauth sway libwayland-client0 libgcc-s1 \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --create-home tester
USER tester
WORKDIR /tmp
