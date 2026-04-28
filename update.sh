set -e

git pull --recurse-submodules
git submodule update --init --recursive

cargo build -r

cd web
bun install
bun vite build
cd ..

pm2 restart qxp-app
