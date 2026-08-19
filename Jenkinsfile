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
