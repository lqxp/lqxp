const path = require("path");

const projectRoot = __dirname;

module.exports = {
  apps: [
    {
      name: "qxp-app",
      script: "./target/release/qxprotocol",
      cwd: projectRoot,
      exec_mode: "fork",
      instances: 1,
      max_memory_restart: "4G",
      autorestart: true,
      watch: false,
      env: {
        PRODUCTION: "1",
        RUST_LOG: "info",
        QXP_ROOT: projectRoot,
      },
      error_file: path.join(require("os").homedir(), ".pm2/logs/qxchat-error.log"),
      out_file: path.join(require("os").homedir(), ".pm2/logs/qxchat-out.log"),
      merge_logs: true,
    },
  ],
};
