git pull;
cargo build -r;
cd web;
bun install;
bun vite build;
cd ..;
pm2 restart qxp-app;