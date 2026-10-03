#!/bin/sh
# Records demo/fleetdeck.gif and the demo/*.png screenshots from demo.tape.
#
# It builds fleetdeck and runs vhs in Docker, so you only need Docker.
# The demo fleet is made-up data from demo/homes, and demo/bin replaces ssh,
# gh and hostname, so the recording reads no real home and calls no network.
set -eu
cd "$(dirname "$0")/.."
docker build -f demo/Dockerfile -t fleetdeck-demo .
docker run --rm -v "$PWD:/vhs" -w /vhs fleetdeck-demo demo/demo.tape
ls -l demo/fleetdeck.gif demo/*.png
