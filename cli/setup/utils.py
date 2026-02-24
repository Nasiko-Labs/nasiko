import time
from kubernetes import client, config
import os
import sys
import platform
import subprocess
import shutil
import tarfile
import urllib.request
from pathlib import Path


def get_tools_dir() -> Path:
    """
    Returns the cross-platform tools directory in the user's home directory.

    Location: ~/.nasiko/bin
    - Linux: /home/<user>/.nasiko/bin
    - macOS: /Users/<user>/.nasiko/bin
    - Windows: C:\\Users\\<user>\\.nasiko\\bin

    Creates the directory if it doesn't exist.
    """
    tools_dir = Path.home() / ".nasiko" / "bin"
    tools_dir.mkdir(parents=True, exist_ok=True)
    return tools_dir


def ensure_helm():
    """
    Ensures Helm is available. If not, downloads it to ~/.nasiko/bin
    and adds it to the system PATH for this session.
    """
    # Define local tool path
    tools_dir = get_tools_dir()
    helm_path = tools_dir / ("helm.exe" if platform.system() == "Windows" else "helm")

    # 1. Check if Helm is already installed globally
    if shutil.which("helm"):
        return

    # 2. Check if we already downloaded it
    if helm_path.exists():
        _add_to_path(str(tools_dir))
        return

    print("⚙️  Helm not found. Downloading portable binary...")

    # 3. Download Logic
    system = platform.system().lower()  # linux, darwin, windows
    machine = platform.machine().lower()

    # Map architecture
    if machine in ["arm64", "aarch64"]:
        arch = "arm64"
    else:
        arch = "amd64"

    version = "v3.14.0"
    url = f"https://get.helm.sh/helm-{version}-{system}-{arch}.tar.gz"

    tar_path = tools_dir / "helm.tar.gz"

    print(f"   Downloading from {url}...")
    urllib.request.urlretrieve(url, str(tar_path))

    with tarfile.open(str(tar_path), "r:gz") as tar:
        # Extract specifically the binary
        member_name = f"{system}-{arch}/helm" + (".exe" if system == "windows" else "")
        member = tar.getmember(member_name)
        member.name = os.path.basename(member.name)  # Flatten path
        tar.extract(member, path=str(tools_dir))

    # Cleanup
    tar_path.unlink()
    if system != "windows":
        helm_path.chmod(0o755)

    # 4. Add to PATH so subprocess.run(["helm"]) works
    _add_to_path(str(tools_dir))
    print(f"✅ Helm setup complete. Installed to {tools_dir}")

def _add_to_path(path):
    """Updates PATH environment variable for the current process only"""
    os.environ["PATH"] = path + os.pathsep + os.environ["PATH"]


def ensure_terraform():
    """
    Ensures Terraform is available. If not, downloads it to ~/.nasiko/bin
    and adds it to the system PATH for this session.
    """
    # 1. Check if globally installed
    if shutil.which("terraform"):
        return

    # 2. Setup local tool paths
    tools_dir = get_tools_dir()
    exe_name = "terraform.exe" if platform.system() == "Windows" else "terraform"
    tf_path = tools_dir / exe_name

    # 3. Check if already downloaded
    if tf_path.exists():
        _add_to_path(str(tools_dir))
        return

    print("⚙️  Terraform not found. Downloading portable binary...")

    # 4. Detect OS/Arch for download URL
    system = platform.system().lower()  # linux, darwin, windows
    machine = platform.machine().lower()  # amd64, x86_64, arm64

    # Map python platform.machine() to terraform arch strings
    if machine in ["x86_64", "amd64"]:
        arch = "amd64"
    elif machine in ["arm64", "aarch64"]:
        arch = "arm64"
    else:
        print(f"❌ Unsupported architecture: {machine}")
        sys.exit(1)

    # Pin a stable version (Avoids unexpected breaking changes from 'latest')
    version = "1.9.8"
    url = f"https://releases.hashicorp.com/terraform/{version}/terraform_{version}_{system}_{arch}.zip"

    try:
        zip_path = tools_dir / "terraform.zip"

        # Download
        print(f"   Downloading from {url}...")
        urllib.request.urlretrieve(url, str(zip_path))

        # Extract
        import zipfile
        with zipfile.ZipFile(str(zip_path), 'r') as zip_ref:
            zip_ref.extractall(str(tools_dir))

        # Cleanup
        zip_path.unlink()

        # Make executable on Linux/Mac
        if system != "windows":
            tf_path.chmod(0o755)

        # 5. Add to PATH
        _add_to_path(str(tools_dir))
        print(f"✅ Terraform {version} installed to {tools_dir}")

    except Exception as e:
        print(f"❌ Failed to download Terraform: {e}")
        sys.exit(1)

def ensure_doctl():
    """
    Ensures DigitalOcean CLI (doctl) is available. If not, downloads it to ~/.nasiko/bin
    and adds it to the system PATH for this session.
    """
    if shutil.which("doctl"):
        return

    tools_dir = get_tools_dir()
    doctl_path = tools_dir / ("doctl.exe" if platform.system() == "Windows" else "doctl")

    if doctl_path.exists():
        _add_to_path(str(tools_dir))
        return

    print("⚙️  doctl not found. Downloading portable binary...")

    system = platform.system().lower()  # linux, darwin, windows
    machine = platform.machine().lower()  # amd64, x86_64, arm64

    # Map arch
    arch = "amd64"
    if machine in ["arm64", "aarch64"]:
        arch = "arm64"

    version = "1.101.0"
    filename = f"doctl-{version}-{system}-{arch}.{'zip' if system == 'windows' else 'tar.gz'}"
    url = f"https://github.com/digitalocean/doctl/releases/download/v{version}/{filename}"

    try:
        archive_path = tools_dir / filename

        print(f"   Downloading from {url}...")
        urllib.request.urlretrieve(url, str(archive_path))

        # Extract
        if filename.endswith("zip"):
            import zipfile
            with zipfile.ZipFile(str(archive_path), 'r') as z:
                z.extractall(str(tools_dir))
        else:
            with tarfile.open(str(archive_path), "r:gz") as t:
                t.extractall(path=str(tools_dir))

        archive_path.unlink()

        if system != "windows":
            doctl_path.chmod(0o755)

        _add_to_path(str(tools_dir))
        print(f"✅ doctl setup complete. Installed to {tools_dir}")

    except Exception as e:
        print(f"❌ Failed to install doctl: {e}")
        sys.exit(1)

def ensure_kubectl():
    """
    Ensures kubectl is available. If not, downloads it to ~/.nasiko/bin
    and adds it to the system PATH for this session.
    """
    # 1. Check if globally installed
    if shutil.which("kubectl"):
        return

    # 2. Setup local tool paths
    tools_dir = get_tools_dir()
    exe_name = "kubectl.exe" if platform.system() == "Windows" else "kubectl"
    kubectl_path = tools_dir / exe_name

    # 3. Check if already downloaded
    if kubectl_path.exists():
        _add_to_path(str(tools_dir))
        return

    print("⚙️  kubectl not found. Downloading latest stable version...")

    # 4. Detect OS/Arch
    system = platform.system().lower()  # linux, darwin, windows
    machine = platform.machine().lower()

    # Map python platform.machine() to kubectl arch strings
    if machine in ["x86_64", "amd64"]:
        arch = "amd64"
    elif machine in ["arm64", "aarch64"]:
        arch = "arm64"
    else:
        print(f"❌ Unsupported architecture: {machine}")
        sys.exit(1)

    try:
        # Get latest stable version
        version_url = "https://dl.k8s.io/release/stable.txt"
        version = urllib.request.urlopen(version_url).read().decode('utf-8').strip()

        # Construct download URL
        url = f"https://dl.k8s.io/release/{version}/bin/{system}/{arch}/kubectl"
        if system == "windows":
            url += ".exe"

        # Download directly (kubectl is a single binary, no extraction needed!)
        print(f"   Downloading kubectl {version} from {url}...")
        urllib.request.urlretrieve(url, str(kubectl_path))

        # Make executable on Linux/Mac
        if system != "windows":
            kubectl_path.chmod(0o755)

        # Add to PATH
        _add_to_path(str(tools_dir))
        print(f"✅ kubectl {version} installed to {tools_dir}")

    except Exception as e:
        print(f"❌ Failed to download kubectl: {e}")
        sys.exit(1)

def ensure_aws_cli():
    """
    Ensures AWS CLI v2 is available. If not, downloads it to ~/.nasiko/bin
    and adds it to the system PATH for this session.
    On Windows: Warns user (MSI installation is hard to automate portably).
    On macOS: Warns user (PKG installation requires manual install).
    """
    if shutil.which("aws"):
        return

    tools_dir = get_tools_dir()
    aws_exe = tools_dir / ("aws.exe" if platform.system() == "Windows" else "aws")

    if aws_exe.exists():
        _add_to_path(str(tools_dir))
        return

    system = platform.system().lower()
    machine = platform.machine().lower()

    if system == "windows":
        print("❌ AWS CLI not found. On Windows, please install the AWS CLI MSI manually.")
        print("   Download from: https://awscli.amazonaws.com/AWSCLIV2.msi")
        sys.exit(1)

    if system == "darwin":
        print("❌ AWS CLI not found. On macOS, please install AWS CLI manually.")
        print("   Using Homebrew: brew install awscli")
        print("   Or download from: https://awscli.amazonaws.com/AWSCLIV2.pkg")
        sys.exit(1)

    print("⚙️  AWS CLI not found. Downloading and installing locally (this may take a minute)...")

    # Linux logic - support both x86_64 and arm64
    if machine in ["arm64", "aarch64"]:
        url = "https://awscli.amazonaws.com/awscli-exe-linux-aarch64.zip"
    else:
        url = "https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip"

    zip_name = "awscliv2.zip"

    try:
        zip_path = tools_dir / zip_name

        # 1. Download
        print(f"   Downloading from {url}...")
        urllib.request.urlretrieve(url, str(zip_path))

        # 2. Unzip
        import zipfile
        with zipfile.ZipFile(str(zip_path), 'r') as z:
            z.extractall(str(tools_dir))  # Extracts to tools_dir/aws

        # 3. Run Install Script
        # We install to tools_dir/aws-install and link bin to tools_dir/
        install_script = tools_dir / "aws" / "install"
        install_dir = tools_dir / "aws-install"

        subprocess.run([
            str(install_script),
            "-i", str(install_dir),
            "-b", str(tools_dir)
        ], check=True, stdout=subprocess.DEVNULL)

        # 4. Cleanup
        zip_path.unlink()
        shutil.rmtree(tools_dir / "aws")  # Remove the source installer folder

        _add_to_path(str(tools_dir))
        print(f"✅ AWS CLI setup complete. Installed to {tools_dir}")

    except Exception as e:
        print(f"❌ Failed to install AWS CLI: {e}")
        sys.exit(1)

def setup_terraform_modules(source: str = None, force: bool = False) -> Path:
    """
    Set up Terraform modules in ~/.nasiko/terraform/.

    Extracts bundled Terraform modules from the CLI package to the user's
    home directory. Modules are only extracted once unless force=True.

    Args:
        source: Optional custom source directory (overrides bundled modules)
        force: If True, overwrite existing modules

    Returns:
        Path to the terraform modules directory

    Raises:
        FileNotFoundError: If modules cannot be found or extracted
    """
    from .config import get_default_terraform_dir

    dest = get_default_terraform_dir()

    # Check if modules already exist
    aws_exists = (dest / "aws" / "main.tf").exists()
    do_exists = (dest / "digitalocean" / "doks.tf").exists()

    if aws_exists and do_exists and not force:
        return dest

    # Use custom source if provided
    if source:
        source_path = Path(source).resolve()
        if not source_path.exists():
            raise FileNotFoundError(f"Source directory not found: {source_path}")
        return _copy_terraform_from_source(source_path, dest, force)

    # Extract bundled modules from package
    return _extract_bundled_modules(dest, force)


def _extract_bundled_modules(dest: Path, force: bool = False) -> Path:
    """Extract bundled Terraform modules from the CLI package."""
    import importlib.resources as resources

    providers = {
        "aws": "setup.terraform.aws",
        "digitalocean": "setup.terraform.digitalocean",
    }

    for provider, package in providers.items():
        provider_dest = dest / provider
        provider_dest.mkdir(parents=True, exist_ok=True)

        try:
            # Python 3.9+ API
            files = resources.files(package)
            for item in files.iterdir():
                if item.name.endswith(".tf"):
                    dest_file = provider_dest / item.name
                    if dest_file.exists() and not force:
                        continue
                    dest_file.write_text(item.read_text())
        except (ModuleNotFoundError, TypeError, AttributeError):
            # Fallback: try reading from package directory
            try:
                pkg_dir = Path(__file__).parent / "terraform" / provider
                if pkg_dir.exists():
                    for tf_file in pkg_dir.glob("*.tf"):
                        dest_file = provider_dest / tf_file.name
                        if dest_file.exists() and not force:
                            continue
                        shutil.copy2(tf_file, dest_file)
                else:
                    raise FileNotFoundError(f"Bundled {provider} modules not found")
            except Exception as e:
                raise FileNotFoundError(
                    f"Failed to extract {provider} modules: {e}\n"
                    f"Try: nasiko setup k8s init-modules --source /path/to/terraform"
                )

    # Verify extraction
    if not (dest / "aws" / "main.tf").exists():
        raise FileNotFoundError("AWS modules not extracted correctly")
    if not (dest / "digitalocean" / "doks.tf").exists():
        raise FileNotFoundError("DigitalOcean modules not extracted correctly")

    print(f"✅ Terraform modules ready: {dest}")
    return dest


def _copy_terraform_from_source(source: Path, dest: Path, force: bool = False) -> Path:
    """Copy Terraform modules from a custom source directory."""
    providers = ["aws", "digitalocean"]

    for provider in providers:
        provider_src = source / provider
        provider_dest = dest / provider

        if not provider_src.exists():
            print(f"⚠️  Provider modules not found: {provider_src}")
            continue

        provider_dest.mkdir(parents=True, exist_ok=True)

        tf_files = list(provider_src.glob("*.tf"))
        if not tf_files:
            print(f"⚠️  No .tf files found in {provider_src}")
            continue

        for tf_file in tf_files:
            dest_file = provider_dest / tf_file.name
            if dest_file.exists() and not force:
                continue
            shutil.copy2(tf_file, dest_file)

        print(f"✅ Copied {provider} modules ({len(tf_files)} files)")

    return dest


def get_service_external_ip(namespace, service_name, timeout=300):
    """
    Polls Kubernetes for the external IP or Hostname of a LoadBalancer service.
    """
    # Ensure config is loaded
    try:
        kubeconfig = os.environ.get("KUBECONFIG")
        if kubeconfig:
            config.load_kube_config(config_file=kubeconfig)
        else:
            config.load_kube_config()
        v1 = client.CoreV1Api()
    except Exception as e:
        print(f"❌ Error loading kubeconfig: {e}")
        return "Unknown (Config Error)"

    print(f"[dim]⏳ Waiting for LoadBalancer IP: {service_name} (up to {timeout}s)...[/]")
    
    # Initial wait to allow service status to propagate
    time.sleep(5)
    
    start_time = time.time()
    while time.time() - start_time < timeout:
        try:
            svc = v1.read_namespaced_service(name=service_name, namespace=namespace)
            
            # Check if LoadBalancer Ingress is populated
            if svc.status and svc.status.load_balancer and svc.status.load_balancer.ingress:
                ingress = svc.status.load_balancer.ingress[0]
                
                # Handle both object (attribute access) and dict (dictionary access)
                # Some client versions or configurations might return dicts
                address = None
                if hasattr(ingress, 'ip'):
                    address = ingress.ip or getattr(ingress, 'hostname', None)
                elif isinstance(ingress, dict):
                    address = ingress.get('ip') or ingress.get('hostname')
                
                if address:
                    print(f"✅ LoadBalancer IP/Hostname found: {address}")
                    return address
        except Exception as e:
            # Uncomment for deep debugging if needed
            # print(f"[debug] Error polling service {service_name}: {e}")
            pass
        
        time.sleep(5) # Wait 5s before retry

    return "Pending (Check via kubectl)"
