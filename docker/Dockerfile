FROM debian:buster
ARG DEBIAN_FRONTEND=noninteractive
RUN apt update
RUN yes | apt install zsh neovim tmux git build-essential curl g++ cmake curl ninja-build python3 pkg-config
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y
ENV PATH=$PATH:/root/.cargo/bin
