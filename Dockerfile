FROM ubuntu:noble

RUN apt update && \
    apt upgrade --yes

COPY target/release/assertion-executor /usr/local/bin/assertion-executor