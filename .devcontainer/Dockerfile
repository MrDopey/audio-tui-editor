FROM rust:1.98.0-trixie

ARG USER_ID=1000
ARG GROUP_ID=1000
ARG USERNAME=lovely

RUN $(groupadd -g $GROUP_ID customgroup || true) && \
    useradd -m -s /bin/bash -u $USER_ID -g $GROUP_ID $USERNAME

# Install basic development tools, plus what audioedit needs:
#   ffmpeg           the media backend (decoding, trimming, muxing, probing)
#   libasound2-dev   ALSA headers, required to build the audio output backend
RUN apt update && apt install -y less man-db sudo \
    ffmpeg libasound2-dev pkg-config

# Ensure default `node` user has access to `sudo`
RUN echo $USERNAME ALL=\(root\) NOPASSWD:ALL > /etc/sudoers.d/$USERNAME \
    && chmod 0440 /etc/sudoers.d/$USERNAME

# Set `DEVCONTAINER` environment variable to help with orientation
ENV DEVCONTAINER=true

RUN rustup component add rustfmt

RUN apt-get update && apt-get install -y \
  ffmpeg \
  rsgain

USER $USERNAME

RUN curl -fsSL https://claude.ai/install.sh | bash

