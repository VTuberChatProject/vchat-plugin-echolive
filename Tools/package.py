"""将独立插件构建产物与资源打包为可导入的 vcplug。"""
import argparse
import json
import logging
import os
import pathlib
import platform
import subprocess
import tempfile
import zipfile


def main():
    """读取 Cargo 目标目录及清单，原子写入完整插件包。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=('debug', 'release'), default='release')
    parser.add_argument('--output', type=pathlib.Path)
    arguments = parser.parse_args()
    root = pathlib.Path(__file__).resolve().parents[1]
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1'], cwd=root))
    package = metadata['packages'][0]
    target = next(item for item in package['targets'] if 'cdylib' in item['kind'])
    system = {'Windows': 'windows', 'Linux': 'linux', 'Darwin': 'macos'}[platform.system()]
    prefix, extension = {'windows': ('', 'dll'), 'linux': ('lib', 'so'), 'macos': ('lib', 'dylib')}[system]
    library = f'{prefix}{target["name"]}.{extension}'
    source = pathlib.Path(metadata['target_directory']) / arguments.profile / library
    if not source.is_file():
        raise RuntimeError(f'缺少插件动态库，请先构建: {source}')
    manifest = json.loads((root / 'manifest.json').read_text(encoding='utf-8'))
    machine = platform.machine().lower()
    manifest.update(platform=system, architecture={'amd64': 'x86_64', 'arm64': 'aarch64'}.get(machine, machine),
                    package_version=2, entry=f'bin/{library}')
    output = (arguments.output or root / 'target' / f'{manifest["id"]}.vcplug').resolve()
    if output.suffix != '.vcplug':
        raise ValueError('输出文件扩展名必须为 .vcplug')
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=output.parent, suffix='.vcplug', delete=False) as stream:
            temporary = pathlib.Path(stream.name)
        with zipfile.ZipFile(temporary, 'w', zipfile.ZIP_DEFLATED) as archive:
            archive.writestr('manifest.json', json.dumps(manifest, ensure_ascii=False, indent=2) + '\n')
            archive.write(source, manifest['entry'])
            resources = root / 'resources'
            if resources.exists():
                for item in sorted(resources.rglob('*')):
                    if item.is_symlink():
                        raise ValueError(f'资源包含链接: {item}')
                    if item.is_file():
                        archive.write(item, item.relative_to(root).as_posix())
        os.replace(temporary, output)
        logging.info('插件打包完成: %s', output)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


if __name__ == '__main__':
    logging.basicConfig(level=logging.INFO, format='%(levelname)s %(message)s')
    try:
        main()
    except Exception:
        logging.exception('插件打包失败')
        raise SystemExit(1)
