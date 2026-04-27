module.exports = {
  apps: [
    {
      name: "qxp-app",
      script: "./scripts/start-qxp.sh",
      cwd: __dirname + "/../..",
      interpreter: "bash",
      autorestart: true,
      max_restarts: 10,
      env: {
        RUST_LOG: "info"
      }
    }
  ]
}
