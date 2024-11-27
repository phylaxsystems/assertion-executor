FROM ubuntu:20.04

RUN apt update && \
    apt upgrade --yes

COPY target/debug/assertion-executor /usr/local/bin/assertion-executor