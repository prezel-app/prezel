ALTER TABLE deployments
    ADD COLUMN config_visibility TEXT;

ALTER TABLE deployments
    ADD COLUMN config_build_backend TEXT;

ALTER TABLE deployments
    ADD COLUMN config_dockerfile_path TEXT;
