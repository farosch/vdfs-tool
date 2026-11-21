#!/usr/bin/env python3

import datetime
import re
import subprocess
import sys


def next_release_tag(tags, now=None):
    now = now or datetime.datetime.now(datetime.timezone.utc)
    year = now.year
    month = now.month
    pattern = re.compile(rf"^v{year}\.{month}\.([1-9]\d*)$")

    increments = [int(match.group(1)) for tag in tags if (match := pattern.match(tag))]
    increment = max(increments, default=0) + 1
    return f"v{year}.{month}.{increment}"


def main():
    try:
        tags = subprocess.run(
            ["git", "tag", "--list"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.splitlines()
    except subprocess.CalledProcessError as error:
        print(f"failed to list Git tags: {error}", file=sys.stderr)
        return 1

    print(next_release_tag(tags))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
