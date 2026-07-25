module.exports = {
  apps: [
    {
      name: 'qxp-app',
      port: 4560,
      exec_mode: 'fork',
      instances: 1,
      script: 'cargo build -r ; ./target/release/qxprotocol',
      error_file: require('path').join(require('os').homedir(), '.pm2/logs/qxchat-error.log'),
      out_file: require('path').join(require('os').homedir(), '.pm2/logs/qxchat-out.log'),
      merge_logs: true,
      max_memory_restart: '2GB',
      watch: false,
      autorestart: true,
    },
  ],
}
