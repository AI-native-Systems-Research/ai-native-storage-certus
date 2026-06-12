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
    stage('KV-Offload Calibrate') {
      when {
        expression { !fileExists('/var/lib/certus-ci/baselines.json') }
      }
      steps {
        sh '''
          source /home/bdh/kvconn-trace/.venv/bin/activate
          cd benchmarks/kv-offload-replay
          python ci/regression_check.py --calibrate --connector certus \
            --trace traces/sharegpt-multiturn/500convs-64g \
            --num-blocks 32768
        '''
      }
    }
    stage('KV-Offload Regression') {
      steps {
        sh '''
          source /home/bdh/kvconn-trace/.venv/bin/activate
          cd benchmarks/kv-offload-replay
          python ci/regression_check.py --connector certus \
            --trace traces/sharegpt-multiturn/500convs-64g \
            --num-blocks 32768
        '''
      }
      post {
        failure {
          sh '''
            source /home/bdh/kvconn-trace/.venv/bin/activate
            cd benchmarks/kv-offload-replay
            python ci/regression_check.py --connector cpu \
              --trace traces/sharegpt-multiturn/500convs-64g \
              --num-blocks 32768 || true
            python ci/regression_check.py --connector fs \
              --trace traces/sharegpt-multiturn/500convs-64g \
              --num-blocks 32768 || true
          '''
        }
      }
    }
  }
}
