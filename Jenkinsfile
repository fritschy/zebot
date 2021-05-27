pipeline {
    agent any
    
    triggers { pollSCM('H/5 * * * *') }

    stages {
        stage('Build') {
            steps {
                sh 'cargo build'
            }
        }
    
        stage('Test') {
            steps {
                sh 'cargo test --package=irc2'
                sh 'cargo test'
            }
        }
    
        stage('Build-Release') {
            steps {
                sh 'cargo build --release'
            }
        }
    }
}
