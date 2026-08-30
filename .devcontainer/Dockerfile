FROM rust:1.98.0-trixie

ARG USER_ID=1000
ARG GROUP_ID=1000
ARG USERNAME=lovely

RUN $(groupadd -g $GROUP_ID customgroup || true) && \
    useradd -m -s /bin/bash -u $USER_ID -g $GROUP_ID $USERNAME

# Install basic development tools, plus what audioedit needs:
#   ffmpeg              the media backend (decoding, trimming, muxing, probing)
#   libasound2-dev      ALSA headers, required to build the audio output backend
#   libasound2-plugins  the ALSA -> PulseAudio bridge, so playback can leave the
#                       container (see "On macOS (Apple Silicon)" in the README)
#   pulseaudio-utils    pactl/paplay, for checking that bridge by hand
RUN apt update && apt install -y less man-db sudo \
    ffmpeg libasound2-dev pkg-config libasound2-plugins pulseaudio-utils

# Send ALSA's default device to the PulseAudio server named by $PULSE_SERVER.
# `fallback` keeps a real device working if one is ever passed through, and an
# unreachable server is not fatal: audioedit reports no device and stays usable.
RUN printf '%s\n' \
    'pcm.!default {' \
    '    type pulse' \
    '    fallback "sysdefault"' \
    '}' \
    'ctl.!default {' \
    '    type pulse' \
    '    fallback "sysdefault"' \
    '}' > /etc/asound.conf

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

