pipeline {
  agent any
  stages {
    stage('Build Server') {
      steps {
          sh 'pwd'
          sh 'whoami'

        script {
          def status = sh(script: '. ~/.cargo/env ; cargo build', returnStatus: true)
          echo "Server build exit status:-> ${status}"

          if (status != 0) {
            error("Server build failed with status ${status}")
          }
        }
      }
    }
  }
}
