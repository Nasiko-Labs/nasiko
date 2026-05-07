version: '3.8'
services:
  calculator-server:
    container_name: calculator-server
    build: .
    stdin_open: true

