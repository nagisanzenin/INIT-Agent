import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from bootstrap import copy_source,verify_vendor

class Tests(unittest.TestCase):
    def test_vendor_preserves_crate_internal_target_and_checks_every_file(self):
        with tempfile.TemporaryDirectory() as temp:
            top=Path(temp).resolve();source=top/'cc';source.mkdir();(source/'src/target').mkdir(parents=True)
            payload=b'pub enum Target {}\n';(source/'src/target/parser.rs').write_bytes(payload)
            (source/'.cargo-checksum.json').write_text(json.dumps({'files':{'src/target/parser.rs':hashlib.sha256(payload).hexdigest()},'package':'fixture'}))
            dest=top/'vendor/cc';dest.parent.mkdir()
            copy_source(source,dest,full_vendor=True);verify_vendor(dest)
            self.assertEqual((dest/'src/target/parser.rs').read_bytes(),payload)
            (dest/'src/target/parser.rs').write_text('corrupt')
            with self.assertRaises(ValueError):verify_vendor(dest)
    def test_vendor_refuses_symlink_before_copy(self):
        with tempfile.TemporaryDirectory() as temp:
            top=Path(temp).resolve();source=top/'crate';source.mkdir();(source/'escape').symlink_to('/etc/passwd')
            with self.assertRaises(ValueError):copy_source(source,top/'out',full_vendor=True)
            self.assertFalse((top/'out').exists())
    def test_project_generated_target_still_excluded(self):
        with tempfile.TemporaryDirectory() as temp:
            top=Path(temp).resolve();source=top/'source';(source/'target').mkdir(parents=True)
            (source/'target/build').write_text('generated');(source/'lib.rs').write_text('source')
            copy_source(source,top/'out')
            self.assertFalse((top/'out/target').exists());self.assertTrue((top/'out/lib.rs').is_file())

if __name__=='__main__':unittest.main()
