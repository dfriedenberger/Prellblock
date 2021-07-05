FROM rust:latest


RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    build-essential \
    libssl-dev \
    musl-tools \
    && true


RUN apt-get -y install \
             libclang-dev \
             llvm-dev clang uuid-dev \
             telnet \
             tcpdump


#profiling
RUN apt-get -y install linux-tools-4.19 moreutils


COPY . /prellblock
WORKDIR /prellblock

#RUN rustup target add ${TARGET}

RUN RUST_BACKTRACE=full

RUN cargo build --release


#ENTRYPOINT ["/prellblock/target/release/prellblock"]
