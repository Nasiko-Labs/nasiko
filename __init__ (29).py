[build-system]
requires = ["setuptools>=68.0", "wheel"]
build-backend = "setuptools.build_meta"

[project]
name = "nasiko"
version = "0.1.0"
requires-python = ">=3.10"
dependencies = [
    "pydantic>=2.0",
    "httpx>=0.24",
    "fastapi>=0.100",
    "uvicorn>=0.23",
    "tenacity>=8.0",
    "python-multipart>=0.0.6",
    "opentelemetry-api>=1.36.0",
    "opentelemetry-sdk>=1.36.0",
    "opentelemetry-exporter-otlp>=1.36.0",
    "opentelemetry-instrumentation-fastapi>=0.48b0",
    "arize-phoenix>=12.0.0",
    "redis",
]

[project.optional-dependencies]
test = [
    "pytest>=7.0",
]
