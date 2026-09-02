// Kill any certus-server / iops-benchmark left holding the NVMe vfio group by a
// prior stage or a previous (possibly crashed) pipeline run, then wait for the
// device to actually be released. SPDK does not release the vfio group instantly
// on SIGTERM, and a lingering holder makes spdk_nvme_probe abort the next server
// with "Device or resource busy". The [c]/[i] bracket trick keeps pgrep/pkill
// from matching their own command line.
def freeNvmeDevice() {
  sh '''
    pkill -TERM -f "release/[c]ertus-server" || true
    pkill -TERM -f "[i]ops-benchmark" || true
    for i in $(seq 1 15); do
      pgrep -f "release/[c]ertus-server" >/dev/null 2>&1 || break
      sleep 1
    done
    pkill -KILL -f "release/[c]ertus-server" || true
    pkill -KILL -f "[i]ops-benchmark" || true
    sleep 3
  '''
}

pipeline {
  agent any
  environment {
    LD_LIBRARY_PATH = '${LD_LIBRARYPATH}:/usr/local/lib'
  }
  stages {
    stage('Reap Stale Processes') {
      steps {
        script { freeNvmeDevice() }
      }
    }
    stage('Spec-Sync Gate') {
      // Fail fast, before any build, if a changed component's spec-sync
      // drift-report.md is missing, still shows unresolved drift, or went stale
      // (its src/specs — or the shared interfaces — changed after the last
      // /component-sync-specs run). This runs NO Claude: the analysis happens on
      // the developer's machine, and CI only verifies the committed freshness
      // stamp against a recomputed content hash.
      steps {
        sh '''
          set -eu

          # Diff base: the PR target on multibranch PR builds, else unstable.
          # (On the mainline itself the merge-base is HEAD, so nothing is checked.)
          #
          # This gate must fail CLOSED. A multibranch checkout often fetches only
          # the built branch's refspec, so refs/remotes/origin/<target> may not
          # exist in the workspace. If we let an unresolvable base silently degrade
          # to an empty diff, the gate passes without checking anything and stale
          # spec-sync reports sail through. So resolve the base explicitly, prefer
          # the remote-tracking ref but fall back to the FETCH_HEAD we just fetched,
          # and abort the gate outright if neither is available.
          target="${CHANGE_TARGET:-unstable}"
          git fetch -q origin "$target" || {
            echo "Spec-Sync Gate: could not fetch origin/$target — cannot establish a diff base; failing closed."
            exit 1
          }
          if git rev-parse --verify -q "origin/$target" >/dev/null; then
            base="origin/$target"
          elif git rev-parse --verify -q FETCH_HEAD >/dev/null; then
            base="FETCH_HEAD"
          else
            echo "Spec-Sync Gate: base ref for '$target' is unresolvable in this workspace; failing closed."
            exit 1
          fi
          mb="$(git merge-base "$base" HEAD)" || {
            echo "Spec-Sync Gate: no merge-base between $base and HEAD; failing closed."
            exit 1
          }

          # Every directory that carries specs (a component under the gate).
          # Scope is components/ only — lib/ and tools/ crates are not gated
          # (their source layout differs from the components/<name>/src model the
          # hash tool assumes).
          all_spec_dirs() {
            find components -maxdepth 2 -type d -name specs 2>/dev/null \
              | sed 's#/specs$##' | sort -u
          }

          # Select a component only when its *hashed inputs* changed — files under
          # its src/ or specs/. Changes confined to sync scratch (.specify/sync/**),
          # README, or Cargo.toml do not affect the content hash and must not
          # demand a fresh stamp, so they are deliberately excluded here.
          #
          # Compute the diff first and fail closed if it errors (e.g. a bad base);
          # only the grep no-match is tolerated with '|| true', so a genuinely empty
          # change set is distinguished from a broken diff.
          diff_out="$(git diff --name-only "$mb" HEAD)" || {
            echo "Spec-Sync Gate: 'git diff $mb HEAD' failed; failing closed."
            exit 1
          }
          changed="$(echo "$diff_out" \
                     | grep -oE '^components/[^/]+/(src|specs)/' \
                     | grep -oE '^components/[^/]+' | sort -u || true)"

          # Every component's hash folds in components/interfaces, so an
          # interface change can stale reports of components whose own files did
          # not change and would be missed by diff selection. Expand to all.
          if echo "$changed" | grep -qx 'components/interfaces'; then
            echo "interfaces changed → checking all spec'd components"
            targets="$(all_spec_dirs)"
          else
            targets="$changed"
          fi

          [ -n "$targets" ] || { echo "Spec-Sync Gate: no spec'd components changed; nothing to verify."; exit 0; }

          rc=0
          for d in $targets; do
            [ -d "$d/specs" ] || continue          # only dirs with specs are gated
            rpt="$d/.specify/sync/drift-report.md"

            # Grandfather clause (opt-in migration): a component that has never
            # adopted spec-sync — no freshness stamp committed, whether the
            # report is absent entirely or predates the stamp format — is SKIPPED
            # rather than failed. Adoption is opt-in; only the no-stamp case is
            # exempt. A component that HAS a stamp is fully enforced below: an
            # unresolved drift status or a stale hash still fails the gate. The
            # stamp is the sole opt-in signal, so it is read first.
            want="$(grep -m1 '^spec_sync_inputs_sha256:' "$rpt" 2>/dev/null | awk '{print $2}' || true)"
            if [ -z "$want" ]; then
              echo "skip $d: no spec-sync stamp (component has not adopted spec-sync)"
              continue
            fi

            status="$(grep -m1 '^spec_sync_drift_status:' "$rpt" | awk '{print $2}' || true)"
            if [ "$status" != "clean" ]; then
              echo "FAIL $d: spec_sync_drift_status='${status:-<unset>}' (expected 'clean') — resolve drift via /component-sync-specs."
              rc=1; continue
            fi

            have="$(scripts/spec-sync-hash.sh "$d")"
            if [ "$want" != "$have" ]; then
              echo "FAIL $d: stale report — src/specs (or interfaces) changed after last sync."
              echo "         stamped=$want recomputed=$have"
              echo "         re-run '/component-sync-specs $(basename "$d")' and commit the updated drift-report.md."
              rc=1; continue
            fi

            echo "ok   $d"
          done

          [ "$rc" -eq 0 ] || { echo "Spec-Sync Gate failed."; exit 1; }
          echo "Spec-Sync Gate passed."
        '''
      }
    }
    stage('Build Server') {
      steps {
          sh 'pwd'
          sh 'whoami'
          sh '[ -L "./deps/spdk" ] || ln -s "/opt/spdk/" "./deps/spdk"'
          sh '[ -L "./deps/spdk-build" ] || ln -s "/opt/spdk-build/" "./deps/spdk-build"'
          sh '[ -L "./deps/zyre" ] || ln -s "/opt/zyre/" "./deps/zyre"'
          sh '[ -L "./deps/zyre-build" ] || ln -s "/opt/zyre-build/" "./deps/zyre-build"'
          sh 'cd ./kernel/modules/gdrcopy/; make'
        script {
          def status = sh(script: '. ~/.cargo/env ; cargo build', returnStatus: true)
          echo "Server build exit status:-> ${status}"

          if (status != 0) {
            error("Server build failed with status ${status}")
          }
        }
      }
    }
    stage('Hardware-Agnostic Unit Tests') {
      steps {
        sh '. ~/.cargo/env ; cargo t --workspace'
      }
    }
    stage('GPU Unit Tests') {
      steps {
        sh '. ~/.cargo/env ; cargo t --workspace --features gpu'
      }
    }
    stage('SPDK Unit Tests') {
      steps {
        sh '. ~/.cargo/env ; cargo t --workspace --features spdk'
      }
    }
    stage('Benchmarks') {
      steps {
        sh '. ~/.cargo/env ; sleep 3; cargo r -r -p iops-benchmark -- --pci-addr 0000:86:00.0'
      }
    }
    stage('Install Python Dependencies') {
      steps {
        sh 'pip3 install -r apps/python/requirements.txt'
      }
    }
    stage('Integration Tests') {
      steps {
        script {
          sh '. ~/.cargo/env ; cargo r -r -p certus-server -- --device-pci 0000:86:00.0 --shm-path /dev/shm/certus-shmq --channels 32 --format &'
          sh 'for i in $(seq 1 10); do [ -e /dev/shm/certus-shmq ] && break || sleep 5; done'

          def output1 = sh(script: 'cd apps/python && python3 test-promote.py', returnStdout: true).trim()
          echo output1
          if (!output1.contains('PASS')) {
            freeNvmeDevice()
            error("test-promote.py did not output PASS")
          }

          def output2 = sh(script: 'cd apps/python && python3 test-tier-batch.py', returnStdout: true).trim()
          echo output2
          freeNvmeDevice()
          if (!output2.contains('PASS: All tiers returned expected results')) {
            error("test-tier-batch.py did not output expected PASS message")
          }
        }
      }
    }
    stage('vLLM Connector') {
      steps {
        script {
          // The Integration Tests stage's certus-server may not have released
          // the NVMe vfio group yet; free the device before we attach it.
          freeNvmeDevice()

          // certus-server-yaml is not a default member, so this is its first
          // *release* build in the pipeline. Build it up front (blocking) —
          // otherwise the backgrounded launch below races the mailbox wait, and
          // a cold build that overruns leaves the test attaching to a shm file
          // that does not exist yet.
          def build = sh(script: '. ~/.cargo/env ; CERTUS_PROFILE=full cargo build -r -p certus-server-yaml', returnStatus: true)
          if (build != 0) {
            error("certus-server-yaml release build failed with status ${build}")
          }

          // Launch the already-built binary, capturing stdout/stderr so a
          // startup crash (e.g. a full-profile component failing to init) is
          // visible in the console instead of silently yielding "no mailbox".
          sh '. ~/.cargo/env ; CERTUS_PROFILE=full target/release/certus-server-yaml --memory-tier-size 256M --shm-path /dev/shm/certus-shmq --channels 32 --format --device-pci 0000:86:00.0 > /tmp/certus-server-yaml.log 2>&1 &'
          sh 'for i in $(seq 1 60); do [ -e /dev/shm/certus-shmq ] && break || sleep 2; done'

          if (sh(script: '[ -e /dev/shm/certus-shmq ]', returnStatus: true) != 0) {
            echo '=== certus-server-yaml did not create the mailbox — server log follows ==='
            sh 'cat /tmp/certus-server-yaml.log || true'
            freeNvmeDevice()
            error('certus-server-yaml failed to start (no /dev/shm/certus-shmq)')
          }

          def status = sh(script: 'cd apps/python && python3 test-offloading-spec.py --memory-tier-size 256M', returnStatus: true)
          echo "test-offloading-spec.py exit status: ${status}"
          echo '=== certus-server-yaml log ==='
          sh 'cat /tmp/certus-server-yaml.log || true'
          freeNvmeDevice()

          if (status != 0) {
            error("test-offloading-spec.py failed with exit status ${status}")
          }
        }
      }
    }
  }
}
