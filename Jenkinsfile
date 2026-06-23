pipeline {
  agent any
  environment {
    LD_LIBRARY_PATH = '${LD_LIBRARYPATH}:/usr/local/lib'
  }
  stages {
    stage('Build Server') {
      steps {
          sh 'pwd'
          sh 'whoami'
          sh '[ -L "./deps/spdk" ] || ln -s "/opt/spdk/" "./deps/spdk"'
          sh '[ -L "./deps/spdk-build" ] || ln -s "/opt/spdk-build/" "./deps/spdk-build"'
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
          sh '. ~/.cargo/env ; cargo r -r -p certus-server -- --device-pci 0000:86:00.0 --format &'
          sh 'for i in $(seq 1 60); do nc -z localhost 50051 && break || sleep 2; done'

          def output1 = sh(script: 'cd apps/python && python3 test-promote.py', returnStdout: true).trim()
          echo output1
          if (!output1.contains('PASS')) {
            sh 'pkill -f certus-server || true'
            error("test-promote.py did not output PASS")
          }

          def output2 = sh(script: 'cd apps/python && python3 test-tier-batch.py', returnStdout: true).trim()
          echo output2
          sh 'pkill -f certus-server || true'
          if (!output2.contains('PASS: All tiers returned expected results')) {
            error("test-tier-batch.py did not output expected PASS message")
          }
        }
      }
    }
  }
}
