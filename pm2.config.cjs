module.exports = {
  apps: [
    {
      name: "qxp-app",
      script: "./target/release/qxprotocol",
      exec_mode: "fork",
      instances: 1,
      max_memory_restart: "4G",
      autorestart: true,
      watch: false,
      error_file: require("path").join(require("os").homedir(), ".pm2/logs/qxchat-error.log"),
      out_file: require("path").join(require("os").homedir(), ".pm2/logs/qxchat-out.log"),
      merge_logs: true,
    },
  ],
};