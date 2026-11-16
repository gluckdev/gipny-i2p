"""Exercise the vcpkg wrapper with bounded local tool fixtures, no network."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

HELPER = Path(__file__).resolve().parents[2] / '.github/scripts/vcpkg-i2p-deps.sh'
PIN = '9eec002df1cd109f8f104cd966a763daf9751489'


class DependenciesTests(unittest.TestCase):
    def run_fixture(self, arch, target=None, partial=False):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            tools=root/'tools'; tools.mkdir()
            ports=root/'gipny-vcpkg-ports'; (ports/'.git').mkdir(parents=True)
            (ports/'triplets').mkdir()
            for name in ('x64','arm64'):
                (ports/'triplets'/f'{name}-windows-static-md.cmake').write_text('fixture')
            scripts={
                'git': '#!/bin/sh\ncase "$*" in *cat-file*) echo commit;; *rev-parse*) echo '+PIN+';; esac\n',
                'cygpath':'#!/bin/sh\nprintf "%s\\n" "$2"\n',
                'vcpkg':'''#!/bin/bash
set -eu
printf '%s\\n' "$@" > "$FIXTURE_ARGS"
while (($#)); do
 case "$1" in --triplet) triplet=$2; shift;; --x-install-root=*) dest=${1#*=};; esac
 shift
done
mkdir -p "$dest/$triplet/lib" "$dest/$triplet/include"
touch "$dest/$triplet/lib/boost_program_options-fixture.lib" "$dest/$triplet/lib/boost_atomic-fixture.lib" "$dest/$triplet/lib/zs.lib"
'''}
            for name, content in scripts.items():
                path=tools/name; path.write_text(content); path.chmod(0o755)
            install=root/'installed'
            if partial: (install/f'{arch}-windows-static-md'/'lib').mkdir(parents=True)
            env=dict(os.environ, PATH=str(tools)+os.pathsep+os.environ['PATH'], RUNNER_TEMP=str(root),
                     GITHUB_ENV=str(root/'env'),GIPNY_WINDOWS_ARCH=arch,GIPNY_WINDOWS_HOST_ARCH=arch,
                     VCPKG_INSTALLATION_ROOT=str(tools),FIXTURE_ARGS=str(root/'args'))
            result=subprocess.run(['bash',str(HELPER),str(install),target or arch],env=env,text=True,capture_output=True)
            return result, (root/'args').read_text() if (root/'args').exists() else '', (root/'env').read_text() if (root/'env').exists() else ''

    def test_matching_native_targets_and_host_tools(self):
        for arch in ('x64','arm64'):
            with self.subTest(arch=arch):
                result,args,env=self.run_fixture(arch)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertIn(f'--triplet\n{arch}-windows-static-md\n',args)
                self.assertIn(f'--host-triplet\n{arch}-windows\n',args)
                self.assertIn(f'GIPNY_VCPKG_TRIPLET={arch}-windows-static-md',env)
                self.assertIn(f'/{arch}-windows-static-md/lib',env)

    def test_partial_cache_is_reconciled(self):
        result,args,_=self.run_fixture('arm64',partial=True)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn('install\n',args)

    def test_target_mismatch_or_unknown_fails_before_dependencies(self):
        for target in ('x64','x86','../../x64'):
            with self.subTest(target=target):
                result,args,_=self.run_fixture('arm64',target=target)
                self.assertNotEqual(result.returncode,0)
                self.assertEqual(args,'')


if __name__ == '__main__': unittest.main()
