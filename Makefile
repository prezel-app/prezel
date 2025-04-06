db:
	cargo sqlx database reset --database-url=sqlite:src.db -y

openapi-hash:
	@sha1sum docs/public/openapi.json

openapi:
	cp Cargo.toml /tmp/prezel-cargo.backup
	echo '[[bin]]' >> Cargo.toml
	echo 'name = "openapi"' >> Cargo.toml
	echo 'path = "src/openapi.rs"' >> Cargo.toml
	cargo run --release --bin openapi || (make restore && false)
	make restore

restore:
	mv /tmp/prezel-cargo.backup Cargo.toml

openapi-client: openapi
	openapi-generator generate -i docs/public/openapi.json -g rust -o ./client
