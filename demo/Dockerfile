# Records the README demo with vhs. Run demo/record.sh instead of using this
# file directly.
FROM rust:1-trixie AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p fleetdeck

FROM ghcr.io/charmbracelet/vhs:latest
COPY --from=build /src/target/release/fleetdeck /usr/local/bin/fleetdeck
