pipeline {
  agent any
  stages {
    stage('Build Server') {
      steps {
          sh 'pwd'
          sh 'whoami'
          sh 'ln -sf /opt/spdk-build/ ./deps/spdk-build'
          sh 'ln -sf /opt/spdk/ ./deps/spdk'
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
        sh '. ~/.cargo/env ; cargo r -r -p iops-benchmark'
      }
    }
  }
}
