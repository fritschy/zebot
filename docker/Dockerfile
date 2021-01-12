FROM debian:buster
ARG DEBIAN_FRONTEND=noninteractive
RUN apt update
RUN yes | apt install zsh neovim tmux git build-essential g++ cmake curl ninja-build python3 pkg-config
RUN git clone --depth 1 --branch 1.49.0 https://github.com/rust-lang/rust.git
RUN cd rust && cp config.toml.example config.toml
RUN cd rust && nice ./x.py build && ./x.py install
RUN rm -rf rust
