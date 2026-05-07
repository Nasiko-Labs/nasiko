"""Ingest endpoint -- upload, detect, and deploy MCP server artifacts.

This is the E2E pipeline glue: R1 detection -> R3 manifest generation ->
R2 bridge startup -> R4 orchestration event.

POST /ingest accepts a .zip file, detects the artifact type, and if it's
an MCP server, triggers the full deployment pipeline automatically.
"""

import os
import shutil
import uuid
import zipfile
import logging
from pathlib import Path

from fastapi import APIRouter, UploadFile, File, HTTPException
from fastapi.responses import JSONResponse

from nasiko.app.ingestion.detector import detect_artifact_type
from nasiko.app.ingestion.exceptions import AmbiguousArtifactError, MissingStructureError
from nasiko.app.ingestion.models import ArtifactType

logger = logging.getLogger("nasiko.ingest")

router = APIRouter()


@router.post("/ingest")
async def ingest_artifact(file: UploadFile = File(...)):
    """Upload and process an artifact (agent or MCP server).

    Pipeline:
      1. Validate zip file
      2. Extract to temp directory
      3. Run R1 artifact-type detection
      4. If MCP_SERVER: persist code + trigger R3 manifest generation
      5. Return IngestionRecord with deployment status

    Returns 200 with IngestionRecord on success.
    Returns 400 for invalid uploads.
    Returns 422 for ambiguous artifacts.
    """
    if not file.filename or not file.filename.endswith('.zip'):
        raise HTTPException(400, "Only .zip files are accepted")

    extract_dir = f"/tmp/nasiko/uploads/{uuid.uuid4()}"
    os.makedirs(extract_dir, exist_ok=True)

    try:
        # ZipSlip protection -- validate ALL members before extracting
        with zipfile.ZipFile(file.file) as zf:
            for member in zf.namelist():
                resolved = os.path.realpath(os.path.join(extract_dir, member))
                if not resolved.startswith(os.path.realpath(extract_dir)):
                    raise HTTPException(400, "ZipSlip detected")
            # Only extract after all members are validated
            zf.extractall(extract_dir)

        # R1: Detect artifact type
        record = detect_artifact_type(extract_dir)
        result = record.model_dump(mode='json')

        # Persist extracted code to /tmp/nasiko/{artifact_id}/code/
        # so R2 can later spawn the subprocess. The temp extract_dir
        # is cleaned up in the finally block; this is the persistent copy.
        persist_dir = f"/tmp/nasiko/{record.artifact_id}/code"
        os.makedirs(persist_dir, exist_ok=True)
        for item in os.listdir(extract_dir):
            src = os.path.join(extract_dir, item)
            dst = os.path.join(persist_dir, item)
            if os.path.isdir(src):
                shutil.copytree(src, dst, dirs_exist_ok=True)
            else:
                shutil.copy2(src, dst)

        # Update result with the persisted code path so R2 knows where to find it
        result["code_path"] = persist_dir

        # R3: If MCP server, generate manifest automatically
        if record.artifact_type == ArtifactType.MCP_SERVER and record.entry_point:
            try:
                # Convert the path separators to standard posix so R3 path validation
                # (which does string prefix checking) doesn't fail on Windows.
                source_file = Path(os.path.join(persist_dir, record.entry_point)).as_posix()
                if os.path.exists(source_file):
                    # Generate manifest using R3
                    # Must set NASIKO_SOURCE_ROOT to allow the persist dir
                    old_root = os.environ.get("NASIKO_SOURCE_ROOT")
                    os.environ["NASIKO_SOURCE_ROOT"] = "/tmp/nasiko"
                    try:
                        from nasiko.app.utils.mcp_manifest_generator.generator import generate_manifest
                        manifest = generate_manifest(record.artifact_id, source_file)
                        result["manifest"] = manifest
                        result["manifest_generated"] = True
                        logger.info(
                            f"Generated manifest for {record.artifact_id}: "
                            f"{len(manifest.get('tools', []))} tools, "
                            f"{len(manifest.get('resources', []))} resources, "
                            f"{len(manifest.get('prompts', []))} prompts"
                        )
                    finally:
                        if old_root is not None:
                            os.environ["NASIKO_SOURCE_ROOT"] = old_root
                        else:
                            os.environ.pop("NASIKO_SOURCE_ROOT", None)
            except Exception as e:
                # Manifest generation is best-effort -- don't block the upload
                logger.warning(f"Manifest generation failed for {record.artifact_id}: {e}")
                result["manifest_generated"] = False
                result["manifest_error"] = str(e)

        return JSONResponse(content=result, status_code=200)

    except AmbiguousArtifactError as e:
        raise HTTPException(
            422,
            {"error": "AMBIGUOUS_ARTIFACT", "detail": e.reason}
        )
    except MissingStructureError as e:
        raise HTTPException(
            422,
            {"error": "MISSING_STRUCTURE", "detail": e.reason}
        )
    except zipfile.BadZipFile:
        raise HTTPException(400, "Invalid or corrupted zip file")
    finally:
        # Only remove the temp extraction dir -- persisted code stays
        shutil.rmtree(extract_dir, ignore_errors=True)
