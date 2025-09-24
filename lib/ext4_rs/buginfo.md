# ext4_valid_extent
Disabling the ext4_inode_block_valid check in linux/fs/ext4/block_validity.c allows cat 4G.txt to work normally.

```bash
root@acc:~/ext4_rs# umount tmp
root@acc:~/ext4_rs# mount ./ex4.img ./tmp
root@acc:~/ext4_rs# cd tmp
root@acc:~/ext4_rs/tmp# dmesg -C
root@acc:~/ext4_rs/tmp# cat 4G.txt 
cat: 4G.txt: Input/output error
root@acc:~/sync/ext4_rs/tmp# dmesg
[  314.393336] CPU: 3 PID: 7442 Comm: cat Not tainted 5.15.167.4-microsoft-standard-WSL2+ #8
[  314.393343] Call Trace:
[  314.393347]  <TASK>
[  314.393350]  dump_stack_lvl+0x33/0x46
[  314.393356]  ext4_inode_block_valid.cold+0x5/0x16
[  314.393358]  __ext4_ext_check+0x12f/0x3c0
[  314.393362]  __read_extent_tree_block+0xb2/0x160
[  314.393363]  ext4_find_extent+0x1a7/0x420
[  314.393364]  ext4_ext_map_blocks+0x60/0x17b0
[  314.393366]  ? page_counter_try_charge+0x2f/0xc0
[  314.393368]  ? obj_cgroup_charge_pages+0xc3/0x170
[  314.393369]  ext4_map_blocks+0x1bb/0x5c0
[  314.393371]  ? page_counter_try_charge+0x2f/0xc0
[  314.393372]  ext4_mpage_readpages+0x500/0x760
[  314.393374]  ? __mod_memcg_lruvec_state+0x41/0x80
[  314.393375]  read_pages+0x93/0x270
[  314.393377]  page_cache_ra_unbounded+0x1d4/0x260
[  314.393379]  filemap_get_pages+0xee/0x610
[  314.393381]  filemap_read+0xa7/0x330
[  314.393382]  ? memory_oom_group_write+0x20/0xa0
[  314.393383]  ? __mod_lruvec_page_state+0x53/0xa0
[  314.393384]  ? page_add_new_anon_rmap+0x44/0x110
[  314.393386]  ? __handle_mm_fault+0xe1a/0x13a0
[  314.393387]  ? mmap_region+0x29e/0x620
[  314.393389]  new_sync_read+0x10e/0x1a0
[  314.393392]  vfs_read+0xfa/0x190
[  314.393394]  ksys_read+0x63/0xe0
[  314.393395]  do_syscall_64+0x35/0xb0
[  314.393398]  entry_SYSCALL_64_after_hwframe+0x6c/0xd6
[  314.393401] RIP: 0033:0x7fab4abe8a61
[  314.393403] Code: 00 48 8b 15 b9 73 0e 00 f7 d8 64 89 02 b8 ff ff ff ff eb bd e8 40 c4 01 00 f3 0f 1e fa 80 3d e5 f5 0e 00 00 74 13 31 c0 0f 05 <48> 3d 00 f0 ff ff 77 4f c3 66 0f 1f 44 00 00 55 48 89 e5 48 83 ec
[  314.393405] RSP: 002b:00007ffc5eb01a18 EFLAGS: 00000246 ORIG_RAX: 0000000000000000
[  314.393407] RAX: ffffffffffffffda RBX: 0000000000020000 RCX: 00007fab4abe8a61
[  314.393408] RDX: 0000000000020000 RSI: 00007fab4aa49000 RDI: 0000000000000003
[  314.393408] RBP: 00007ffc5eb01a40 R08: 0000000000000000 R09: 00007fab4ad22440
[  314.393409] R10: 0000000000000022 R11: 0000000000000246 R12: 0000000000020000
[  314.393409] R13: 00007fab4aa49000 R14: 0000000000000003 R15: 0000000000000000
[  314.393410]  </TASK>
[  314.393410] some part of the block region overlaps with some other filesystem metadata blocks.
[  314.393931] ext4_valid_extent fail
[  314.394063] EXT4-fs error (device loop0): ext4_find_extent:943: inode #28: comm cat: pblk 2008180 bad header/extent: invalid extent entries - magic f30a, entries 45, max 340(340), depth 0(0)
```