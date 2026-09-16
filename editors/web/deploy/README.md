# Publishing the editor

The editor's server runs programs, so published it runs them in the
compiler's sandbox — nothing past the process unless `--allow` names it
— inside a container that bounds what the sandbox does not: memory,
processes, and a read-only filesystem with `/tmp` for the scratch.

```sh
# on the box, from a checkout of the repository
docker compose -f editors/web/deploy/docker-compose.yml up -d --build
cp editors/web/deploy/rillsdl.baltavista.com.conf /etc/nginx/conf.d/   # the port-80 half first
certbot certonly --webroot -w /var/www/html -d rillsdl.baltavista.com
nginx -t && systemctl reload nginx
```

The site file is written to that box's shape — a front nginx routes
:443 by SNI to 127.0.0.1:8444 with proxy_protocol, Cloudflare in front of
that — so elsewhere the TLS block is an ordinary `listen 443 ssl`.

The image builds the compiler from the checkout with LLVM 21 (the first
build fetches about 1.5 GB of LLVM and takes a while; later builds reuse
the layers), builds the server with it, and keeps only what runs: the two
binaries, the standard library, the page, a C linker for the programs,
and the examples, which seed the models volume the first time it is
empty. The models live in the `rillsdl-models` volume, so a rebuild keeps
them; `docker run --rm -v rillsdl-models:/models -v $PWD:/out alpine tar
czf /out/models.tgz -C /models .` is a backup.

`--allow files,net` in the compose file's `command` lets the programs
reach those; `--allow all` is the editor as it runs at home, for a server
only trusted people can reach.
