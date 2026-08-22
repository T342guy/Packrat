#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# Lints the bash embedded in the GitHub Actions workflows.
#
# Quite a lot of the release machinery lives inside `run:` blocks, where
# nothing otherwise checks it: a typo there is only discovered by a failed
# release. This extracts each block and runs it past bash and shellcheck.
#
# Actions expressions are replaced with a literal before linting, so a step
# that branches on one should pass it through `env:` rather than interpolating
# it into the script — which is the safer habit anyway.
#
# shellcheck shell=bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

python3 - "$@" <<'PY'
import glob, os, re, subprocess, sys, tempfile, yaml

problems = 0
for path in sorted(glob.glob(".github/workflows/*.yml") + glob.glob(".github/workflows/*.yaml")):
    doc = yaml.safe_load(open(path))
    for job_name, job in (doc.get("jobs") or {}).items():
        for step in job.get("steps") or []:
            script = step.get("run")
            if not script:
                continue
            label = f"{os.path.basename(path)}:{job_name}/{step.get('name', '(unnamed)')}"
            if (step.get("shell") or "bash") not in ("bash", "sh"):
                continue
            stubbed = re.sub(r"\$\{\{[^}]*\}\}", "EXPR", script)
            with tempfile.NamedTemporaryFile("w", suffix=".sh", delete=False) as handle:
                handle.write("#!/usr/bin/env bash\n" + stubbed)
                tmp = handle.name
            try:
                syntax = subprocess.run(["bash", "-n", tmp], capture_output=True, text=True)
                # SC2016/SC2086 fire constantly on heredocs that build markdown.
                lint = subprocess.run(
                    ["shellcheck", "--shell=bash", "--severity=warning",
                     "--exclude=SC2016,SC2086", tmp],
                    capture_output=True, text=True)
            finally:
                os.unlink(tmp)

            if syntax.returncode:
                problems += 1
                print(f"  SYNTAX  {label}\n{syntax.stderr}")
            elif lint.returncode:
                problems += 1
                print(f"  LINT    {label}\n{lint.stdout}")
            else:
                print(f"  ok      {label}")

print(f"\n{problems} problem(s)")
sys.exit(1 if problems else 0)
PY
