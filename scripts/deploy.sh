#!/bin/sh
docker run -u $(id -u) -v $HOME:$HOME -v /data:/data -w $PWD -it -e HOME=$HOME -e RUSTFLAGS= m-buster cargo br
tar --exclude=__pycache__ -c handlers password.txt nag-*.txt -C target/release zebot | ssh apu "tar -C zebot -xv"
