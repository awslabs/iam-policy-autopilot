#!/usr/bin/env python3
"""
Generate a lookup table of AWS operation names → Python snake_case method names.

This script uses botocore's own xform_name() function to produce the authoritative
mapping for all operation names found in the botocore service data directory, plus
all entries pre-populated in botocore's _xform_cache (special cases that may not
appear in the current service data but are handled specially by botocore).

The resulting JSON maps every PascalCase operation name to its Python snake_case
equivalent as boto3 uses it. The caller (build.rs) is responsible for filtering out
entries that convert_case already handles correctly, keeping only the special cases.

Usage:
    python3 generate_python_name_map.py <botocore_root> <botocore_data_dir> <output_json>

Arguments:
    botocore_root     Path to the botocore package root (contains botocore/__init__.py)
    botocore_data_dir Path to botocore/data directory (contains per-service subdirectories)
    output_json       Path to write the output JSON file
"""

import gzip
import json
import os
import sys


def main():
    if len(sys.argv) != 4:
        print(
            f"Usage: {sys.argv[0]} <botocore_root> <botocore_data_dir> <output_json>",
            file=sys.stderr,
        )
        sys.exit(1)

    botocore_root = sys.argv[1]
    botocore_data_dir = sys.argv[2]
    output_json = sys.argv[3]

    # Add botocore root to sys.path so we can import botocore
    sys.path.insert(0, botocore_root)

    try:
        from botocore import _xform_cache, xform_name
    except ImportError as e:
        print(
            f"Error: could not import botocore from {botocore_root}: {e}",
            file=sys.stderr,
        )
        sys.exit(1)

    # Collect all operation names and their xform_name results from service-2.json files,
    # and all waiter names from waiters-2.json files.
    # Use os.walk to match the pattern from the reference implementation.
    operations: dict[str, str] = {}

    for root, dirs, files in os.walk(botocore_data_dir):
        # Process service-2.json for operation names
        service_files = [f for f in files if f.startswith("service-2.json")]
        if service_files:
            service_file = service_files[0]
            if service_file.endswith(".gz"):
                with gzip.open(os.path.join(root, service_file), "rb") as fd:
                    data = json.loads(fd.read().decode("utf-8"))
            else:
                with open(os.path.join(root, service_file), encoding="utf-8") as fd:
                    data = json.loads(fd.read())

            for op_name in data.get("operations", {}).keys():
                op_name_str = str(op_name)
                operations[op_name_str] = xform_name(op_name_str)

        # Process waiters-2.json for waiter names.
        # Waiter names (e.g. "FunctionActiveV2") are passed to boto3's get_waiter()
        # and converted via xform_name, so they need the same treatment as operation names.
        waiter_files = [f for f in files if f.startswith("waiters-2.json")]
        if waiter_files:
            waiter_file = waiter_files[0]
            if waiter_file.endswith(".gz"):
                with gzip.open(os.path.join(root, waiter_file), "rb") as fd:
                    waiter_data = json.loads(fd.read().decode("utf-8"))
            else:
                with open(os.path.join(root, waiter_file), encoding="utf-8") as fd:
                    waiter_data = json.loads(fd.read())

            for waiter_name in waiter_data.get("waiters", {}).keys():
                waiter_name_str = str(waiter_name)
                if waiter_name_str not in operations:
                    operations[waiter_name_str] = xform_name(waiter_name_str)

    # Also include all pre-populated special cases from botocore's _xform_cache.
    # These cover names that may not appear in the current service data but are handled
    # specially by botocore (e.g. PartiQL operations, iSCSI operations, HITs).
    # Only include '_' separator entries (boto3 uses underscores for method names).
    xform_cache_entries = 0
    for (name, sep), result in _xform_cache.items():
        if sep == "_" and name not in operations:
            operations[name] = result
            xform_cache_entries += 1

    # Write the full map as JSON (sorted keys for deterministic output).
    # build.rs will filter this down to only the entries that differ from convert_case.
    output_dir = os.path.dirname(output_json)
    if output_dir:
        os.makedirs(output_dir, exist_ok=True)
    with open(output_json, "w", encoding="utf-8") as f:
        json.dump(operations, f, sort_keys=True, indent=2)
        f.write("\n")

    print(f"Written to {output_json}", file=sys.stderr)


if __name__ == "__main__":
    main()
