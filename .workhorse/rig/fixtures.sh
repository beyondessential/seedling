#!/bin/sh
# Register and install demo apps so the screens have something to show.
# Usage: fixtures.sh <ctl> <demos-dir>
set -eu
ctl="$1"
demos="$2"
for demo in hello-world http-tour workloads-tour actions-and-gates paired-provider paired-consumer; do
    "$ctl" apps create "$demo" "$demos/$demo/"
    "$ctl" apps install "$demo"
done
"$ctl" apps create params-tour "$demos/params-tour/"
