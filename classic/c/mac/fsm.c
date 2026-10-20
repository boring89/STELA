/*
 * Copyright (C) 2026 YuanChi Hsieh
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

/*
 * The external file system's native half (docs/fsm.md). System 7.5's File
 * System Manager (FSM) passes every File Manager request for our volume to
 * the HFS component procedure in emu/mac/fsm.s, which lands here.
 *
 * Some requests need the Mac OS's help: the FSM utility routines (UT...,
 * trap $A824) allocate FCBs, VCBs and working directories and tell how a
 * request named its volume. The model cannot run 68k code from inside an
 * emulated opcode, so a handler asks the 68k half to make the call and
 * returns; once the call has returned, the handler is run again from the
 * start and gets the results of the calls already made, in order ("replay").
 * Handlers therefore make all their changes to Mac memory after the calls
 * they depend on, or make changes that may be repeated.
 *
 * Layouts and selectors: Inside Macintosh: Files (parameter blocks, FCB,
 * VCB, WDCB, drive queue); the FSM's routines and records as Basilisk II's
 * ExtFS uses them (BasiliskII/src/extfs.cpp and include/extfs_defs.h, GPL-2.0
 * or later; only the flow and the offsets were taken, not code).
 */

#include "mac/mac.h"
#include "mac/fsm_code.h"
#include "m68k/vm.h"
#include "shim/k_lib.h"

/* fsm.s, at MAC_FSM_STUB in the stub window */
#define STUB_INSTALL 0x000u
#define STUB_INSTALL_RETURN 0x02Eu
#define STUB_HFS 0x040u
#define STUB_COMM 0x080u
#define STUB_TRAP_SLOT 0x0E0u

#define TRAP_GESTALT 0xA1ADu
#define TRAP_NEWPTR_SYS_CLEAR 0xA71Eu
#define TRAP_ADD_DRIVE 0xA04Eu
#define TRAP_FSM_DISPATCH 0xA0ACu
#define TRAP_HFS_DISPATCH 0xA260u
#define TRAP_FSMGR 0xA824u
#define TRAP_POST_EVENT 0xA02Fu

#define FSID 0x4848u                /* our file system's ID ('HH'): any value no other file system uses */
#define MEDIA_TYPE 0x36384B48u      /* '68KH': VolumeMountInfoHeader.media of our volume */
#define FS_NAME "68k_HAL host"
#define STACK_SIZE 0x4000u
#define DISK_REFNUM (-50)           /* the .Disk driver (emu/mac/disk.c) */
#define BLOCK 512u                  /* reported allocation block size */

#define PENDING 0x7FFF              /* not an OSErr: the handler waits for a trap call */
#define COPY_CHUNK 65536u           /* bytes of a Read or Write passed to the host at a time */

/* Errors (Inside Macintosh: Files) */
#define NO_ERR 0
#define DSK_FUL_ERR (-34)
#define NSV_ERR (-35)
#define BD_NAM_ERR (-37)
#define FN_OPN_ERR (-38)
#define EOF_ERR (-39)
#define POS_ERR (-40)
#define FNF_ERR (-43)
#define WPR_ERR (-44)
#define FLCKD_ERR (-45)
#define FBSY_ERR (-47)
#define OP_WR_ERR (-49)
#define PARAM_ERR (-50)
#define PERM_ERR (-54)
#define RF_NUM_ERR (-51)
#define EXT_FS_ERR (-58)
#define WR_PERM_ERR (-61)
#define DIR_NF_ERR (-120)
#define BAD_MOV_ERR (-122)
#define FSM_UNKNOWN_MESSAGE (-438)
#define AFP_ITEM_NOT_FOUND (-5012)

/* The plug-in's block in the system heap. */
#define DATA_FSD 0u                 /* FSDRec, 196 bytes */
#define DATA_DRIVE 200u             /* per volume, 32 apart: DrvSts, 30 bytes; the drive queue element starts at +6 */
#define DATA_VMI (DATA_DRIVE + MAC_FSM_VOLUMES * 32u)   /* per volume: VolumeMountInfoHeader, then our index (8 bytes) */
#define DATA_PB (DATA_VMI + MAC_FSM_VOLUMES * 8u)       /* the parameter block of PBVolumeMount, 144 bytes */
#define DATA_SCRATCH (DATA_PB + 144u)   /* per frame: MAC_FSM_CALLS slots of SLOT bytes for the calls' outputs */
#define SLOT 64u
#define DATA_ARGS (DATA_SCRATCH + MAC_FSM_FRAMES * MAC_FSM_CALLS * SLOT)
#define ARGS_MAX 32u
#define DATA_SIZE (DATA_ARGS + MAC_FSM_FRAMES * ARGS_MAX)

/* FSDRec / HFSCIRec */
#define FSD_LENGTH 4
#define FSD_VERSION 6
#define FSD_FSID 8
#define FSD_NAME 10
#define FSD_GLOBALS 112
#define FSD_COMM_PROC 116
#define FSD_HFSCI 132
#define CI_MASK 0
#define CI_PROC 4
#define CI_STACK_TOP 12
#define CI_STACK_SIZE 16
#define CI_ID_SECTOR 28
#define FSD_SIZE 196
#define CI_ENABLE 0x80000000u
#define CI_RESOURCE_LOADED 0x00080000u
#define CI_DOES_HFS 0x00800000u

/* DrvSts */
#define DS_DISK_IN_PLACE 3
#define DS_INSTALLED 4
#define DS_QLINK 6
#define DS_QTYPE 10
#define DS_QFSID 16
#define DS_DRIVE_SIZE 18
#define DS_DRIVE_S1 20
#define LM_DRVQ_HDR 0x308u
#define DRVQ_DRIVE 6

/* Parameter blocks */
#define IO_COMPLETION 12
#define IO_NAME_PTR 18
#define IO_VREFNUM 22
#define IO_REFNUM 24
#define IO_PERMSSN 27
#define IO_MISC 28
#define IO_BUFFER 32
#define IO_REQ_COUNT 36
#define IO_ACT_COUNT 40
#define IO_POS_MODE 44
#define IO_POS_OFFSET 46
#define IO_FDIR_INDEX 28
#define IO_FL_ATTRIB 30
#define IO_ACUSER 31
#define IO_FL_FNDR_INFO 32
#define IO_DIR_ID 48
#define IO_FL_ST_BLK 52
#define IO_DR_NM_FLS 52
#define IO_FL_LG_LEN 54
#define IO_FL_PY_LEN 58
#define IO_FL_RST_BLK 62
#define IO_FL_RLG_LEN 64
#define IO_FL_RPY_LEN 68
#define IO_FL_CR_DAT 72
#define IO_FL_MD_DAT 76
#define IO_FL_BK_DAT 80
#define IO_FL_XFNDR_INFO 84
#define IO_FL_PAR_ID 100
#define IO_FL_CLP_SIZ 104
#define IO_NEW_NAME 28              /* CatMove */
#define IO_NEW_DIR_ID 36
#define IO_WD_INDEX 26
#define IO_WD_PROC_ID 28
#define IO_WD_VREFNUM 32
#define IO_WD_DIR_ID 48
#define IO_FCB_INDX 28
#define IO_FCB_FL_NM 32
#define IO_FCB_FLAGS 36
#define IO_FCB_ST_BLK 38
#define IO_FCB_EOF 40
#define IO_FCB_PLEN 44
#define IO_FCB_CRPS 48
#define IO_FCB_VREFNUM 52
#define IO_FCB_CLP_SIZ 54
#define IO_FCB_PAR_ID 58
#define IO_VCR_DATE 30
#define IO_VLS_MOD 34
#define IO_VATRB 38
#define IO_VNM_FLS 40
#define IO_VNM_AL_BLKS 46
#define IO_VAL_BLK_SIZ 48
#define IO_VCLP_SIZ 52
#define IO_VNXT_CNID 58
#define IO_VFR_BLK 62
#define IO_VSIG_WORD 64
#define IO_VDRV_INFO 66
#define IO_VDREFNUM 68
#define IO_VFSID 70
#define IO_VFIL_CNT 82
#define IO_VDIR_CNT 86
#define IO_VFNDR_INFO 90

/* FCB */
#define FCB_FL_NM 0
#define FCB_FLAGS 4
#define FCB_EOF 8
#define FCB_PLEN 12
#define FCB_CRPS 16
#define FCB_VPTR 20
#define FCB_CLMP_SIZE 30
#define FCB_FTYPE 50
#define FCB_DIR_ID 58
#define FCB_CNAME 62
#define FCB_SIZE 94
/* fcbFlags (the high byte of GetFCBInfo's ioFCBFlags: kioFCBWriteBit 8 ... kioFCBModifiedBit 15 in Files.h) */
#define FCB_WRITE 0x01
#define FCB_RESOURCE 0x02
#define FCB_SHARED_WRITE 0x10
#define FCB_FILE_LOCKED 0x20
#define FCB_MODIFIED 0x80

/* The FCB buffer: FCBSPtr points to it, its first word is its length, the FCBs follow (a file's
 * reference number is its FCB's offset in the buffer); FSFCBLen is the length of one (Inside
 * Macintosh: Files, "File Control Blocks"; the FCBSPtr and FSFCBLen addresses: SysEqu.a). */
#define LM_FCBS_PTR 0x34Eu
#define LM_FCB_LEN 0x3F6u

/* VCB */
#define VCB_SIG_WORD 8
#define VCB_CR_DATE 10
#define VCB_LS_MOD 14
#define VCB_ATRB 18
#define VCB_NM_FLS 20
#define VCB_NM_AL_BLKS 26
#define VCB_AL_BLK_SIZ 28
#define VCB_CLP_SIZ 32
#define VCB_NXT_CNID 38
#define VCB_FREE_BKS 42
#define VCB_VN 44
#define VCB_FSID 76
#define VCB_VREFNUM 78
#define VCB_FIL_CNT 114
#define VCB_DIR_CNT 118
#define VA_HARD_LOCK 0x0080

/* WDCB */
#define WD_VCB_PTR 0
#define WD_DIR_ID 4
#define WD_PROC_ID 12

/* GetVolParms (vMAttrib) */
#define GVP_ATTRIB 0x3E0B0000u      /* kNoMiniFndr kNoVNEdit kNoLclSync kTrshOffLine kNoSwitchTo kNoBootBlks kNoSysDir kHasExtFSVol */

/* UTDetermineVol's status */
#define DTMV_FULL_PATHNAME 1
#define DTMV_VREFNUM 2
#define DTMV_WDREFNUM 3
#define DTMV_DRIVENUM 4
#define DTMV_DEFAULT 5

/* FSM utility selectors (trap $A824, D0) */
#define UT_ALLOCATE_FCB 0x00
#define UT_RELEASE_FCB 0x01
#define UT_INDEX_FCB 0x04
#define UT_RESOLVE_FCB 0x05
#define UT_ALLOCATE_VCB 0x06
#define UT_ADD_NEW_VCB 0x07
#define UT_DISPOSE_VCB 0x08
#define UT_ALLOCATE_WDCB 0x0C
#define UT_RELEASE_WDCB 0x0D
#define UT_RESOLVE_WDCB 0x0E
#define UT_SET_DEFAULT_VOL 0x11
#define UT_GET_DEFAULT_VOL 0x12
#define UT_CHECK_WDREFNUM 0x13
#define UT_DETERMINE_VOL 0x1D

/* HFS component selectors (selectCode & $F0FF; bit 9 = the H... variant) */
#define SEL_OPEN 0xA000
#define SEL_CLOSE 0xA001
#define SEL_READ 0xA002
#define SEL_WRITE 0xA003
#define SEL_GET_VOL_INFO 0xA007
#define SEL_CREATE 0xA008
#define SEL_DELETE 0xA009
#define SEL_OPEN_RF 0xA00A
#define SEL_RENAME 0xA00B
#define SEL_GET_FILE_INFO 0xA00C
#define SEL_SET_FILE_INFO 0xA00D
#define SEL_UNMOUNT_VOL 0xA00E
#define SEL_MOUNT_VOL 0xA00F
#define SEL_ALLOCATE 0xA010
#define SEL_GET_EOF 0xA011
#define SEL_SET_EOF 0xA012
#define SEL_FLUSH_VOL 0xA013
#define SEL_LOCK_RANGE 0x0010
#define SEL_UNLOCK_RANGE 0x0011
#define SEL_GET_VOL 0xA014
#define SEL_SET_VOL 0xA015
#define SEL_EJECT 0xA017
#define SEL_GET_FPOS 0xA018
#define SEL_OFFLINE 0xA035
#define SEL_SET_FIL_LOCK 0xA041
#define SEL_RST_FIL_LOCK 0xA042
#define SEL_SET_FIL_TYPE 0xA043
#define SEL_SET_FPOS 0xA044
#define SEL_FLUSH_FILE 0xA045
#define SEL_OPEN_WD 0x0001
#define SEL_CLOSE_WD 0x0002
#define SEL_CAT_MOVE 0x0005
#define SEL_DIR_CREATE 0x0006
#define SEL_GET_WD_INFO 0x0007
#define SEL_GET_FCB_INFO 0x0008
#define SEL_GET_CAT_INFO 0x0009
#define SEL_SET_CAT_INFO 0x000A
#define SEL_SET_VOL_INFO 0x000B
#define SEL_OPEN_DF 0x001A
#define SEL_GET_VOL_PARMS 0x0030
#define SEL_VOLUME_MOUNT 0x0041
#define HFS_BIT 0x0200

/* Communication procedure messages */
#define FFS_NOP 0
#define FFS_GET_ICON 1
#define FFS_ID_DISK 2
#define FFS_LOAD 3
#define FFS_UNLOAD 4
#define FFS_ID_VOL_MOUNT 5

enum
{
    KIND_INSTALL,
    KIND_HFS
};

/* One trap call a handler needs. */
typedef struct
{
    uint16_t trap;
    uint32_t d[5], a[5];        /* D0-D4, A0-A4 */
    uint8_t stack[ARGS_MAX];    /* filled from the end: the first argument pushed is the deepest */
    uint32_t pushed;
    bool pascal_result;         /* a Pascal function: a result word was pushed first */
    uint32_t restore_addr;      /* a long to put back once the call has returned (0 = none) */
    uint32_t restore_value;
} trap_call_t;

typedef struct
{
    mac_t *mac;
    mac_fsm_t *fs;
    mac_fsm_frame_t *f;
    uint32_t depth;
    mac_fsm_volume_t *v;        /* the request's volume */
} ctx_t;

/* ---- Mac memory ---- */

static uint32_t rd8(ctx_t *x, uint32_t a)
{
    return mac_bus_read(x->mac, a, 1);
}

static uint32_t rd16(ctx_t *x, uint32_t a)
{
    return mac_bus_read(x->mac, a, 2);
}

static uint32_t rd32(ctx_t *x, uint32_t a)
{
    return mac_bus_read(x->mac, a, 4);
}

static void wr8(ctx_t *x, uint32_t a, uint32_t v)
{
    mac_bus_write(x->mac, a, 1, v);
}

static void wr16(ctx_t *x, uint32_t a, uint32_t v)
{
    mac_bus_write(x->mac, a, 2, v);
}

static void wr32(ctx_t *x, uint32_t a, uint32_t v)
{
    mac_bus_write(x->mac, a, 4, v);
}

static void wr_bytes(ctx_t *x, uint32_t a, const uint8_t *p, uint32_t n)
{
    for (uint32_t i = 0; i < n; i++)
        wr8(x, a + i, p[i]);
}

static void wr_zero(ctx_t *x, uint32_t a, uint32_t n)
{
    for (uint32_t i = 0; i < n; i++)
        wr8(x, a + i, 0);
}

/* A Pascal string into a buffer of size bytes (the length byte included). */
static void wr_pstring(ctx_t *x, uint32_t a, const uint8_t *s, uint32_t size)
{
    uint32_t n = s[0] < size - 1 ? s[0] : size - 1;

    wr8(x, a, n);
    wr_bytes(x, a + 1, s + 1, n);
}

/* ---- Trap calls and their replay ---- */

static void push16(trap_call_t *c, uint32_t v)
{
    c->pushed += 2;
    c->stack[ARGS_MAX - c->pushed] = (uint8_t)(v >> 8);
    c->stack[ARGS_MAX - c->pushed + 1] = (uint8_t)v;
}

static void push32(trap_call_t *c, uint32_t v)
{
    c->pushed += 4;

    uint8_t *p = &c->stack[ARGS_MAX - c->pushed];

    p[0] = (uint8_t)(v >> 24);
    p[1] = (uint8_t)(v >> 16);
    p[2] = (uint8_t)(v >> 8);
    p[3] = (uint8_t)v;
}

/* The scratch slot of the next trap call (its outputs go there). */
static uint32_t slot(ctx_t *x)
{
    return x->fs->data + DATA_SCRATCH + (x->depth * MAC_FSM_CALLS + x->f->next) * SLOT;
}

/*
 * Make a trap call: true with its D0/A0 if it has already been made (replay),
 * false if the 68k half must make it now (the handler then returns PENDING).
 */
static bool call(ctx_t *x, const trap_call_t *c, uint32_t *d0, uint32_t *a0)
{
    mac_fsm_frame_t *f = x->f;

    if (f->next < f->count)
    {
        *d0 = f->d0[f->next];
        if (a0 != NULL)
            *a0 = f->a0[f->next];
        f->next++;
        return true;
    }
    if (f->count == MAC_FSM_CALLS)
    {
        *d0 = (uint32_t)PARAM_ERR;
        if (a0 != NULL)
            *a0 = 0;
        return true;
    }

    uint32_t args = 0;

    if (c->pushed > 0)
    {
        args = x->fs->data + DATA_ARGS + x->depth * ARGS_MAX;
        wr_bytes(x, args, c->stack + ARGS_MAX - c->pushed, c->pushed);
    }
    for (int i = 0; i < 5; i++)
    {
        m68k_vm_set_reg((m68k_vm_reg_t)(M68K_VM_REG_D0 + i), c->d[i]);
        m68k_vm_set_reg((m68k_vm_reg_t)(M68K_VM_REG_A0 + i), c->a[i]);
    }
    m68k_vm_set_reg(M68K_VM_REG_A0 + 6, args);
    m68k_vm_set_reg(M68K_VM_REG_D0 + 7, c->pushed);
    m68k_vm_set_reg(M68K_VM_REG_D0 + 6, c->pascal_result ? 1 : 0);
    m68k_vm_set_reg(M68K_VM_REG_D0 + 5, 0);
    x->mac->stub[MAC_FSM_STUB + STUB_TRAP_SLOT] = (uint8_t)(c->trap >> 8);
    x->mac->stub[MAC_FSM_STUB + STUB_TRAP_SLOT + 1] = (uint8_t)c->trap;
    x->fs->restore_addr = c->restore_addr;
    x->fs->restore_value = c->restore_value;
    return false;
}

static trap_call_t ut(uint32_t selector)
{
    trap_call_t c;

    k_memset(&c, 0, sizeof c);
    c.trap = TRAP_FSMGR;
    c.d[0] = selector;
    c.pascal_result = true;
    push16(&c, 0);              /* the result */
    return c;
}

static trap_call_t reg_trap(uint16_t trap)
{
    trap_call_t c;

    k_memset(&c, 0, sizeof c);
    c.trap = trap;
    return c;
}

#define CALL(x, c, d0, a0)                                                     \
    do                                                                         \
    {                                                                          \
        if (!call((x), (c), (d0), (a0)))                                       \
            return PENDING;                                                    \
    } while (0)

/* ---- FSM utility routines (their outputs are read from the call's slot) ---- */

/* UTDetermineVol(pb, &status, &moreMatches, &vRefNum, &vcb): how pb names its volume. */
static int ut_determine_vol(ctx_t *x, uint32_t pb, bool hide_name, int16_t *status)
{
    uint32_t s = slot(x), d0;
    trap_call_t c = ut(UT_DETERMINE_VOL);

    push32(&c, pb);
    push32(&c, s);
    push32(&c, s + 2);
    push32(&c, s + 4);
    push32(&c, s + 6);
    if (hide_name && rd32(x, pb + IO_NAME_PTR) != 0)
    {
        /* Indexed requests use ioNamePtr for output: the volume is not named by it. */
        c.restore_addr = pb + IO_NAME_PTR;
        c.restore_value = rd32(x, pb + IO_NAME_PTR);
    }
    if (x->f->next == x->f->count && c.restore_addr != 0)
        wr32(x, pb + IO_NAME_PTR, 0);
    CALL(x, &c, &d0, NULL);
    *status = (int16_t)rd16(x, s);
    return (int16_t)d0;
}

/* UTResolveWDCB(procID, index, vRefNum, &wdcb) */
static int ut_resolve_wdcb(ctx_t *x, uint32_t proc_id, uint32_t index, uint32_t vrefnum, uint32_t *wdcb)
{
    uint32_t s = slot(x), d0;
    trap_call_t c = ut(UT_RESOLVE_WDCB);

    push32(&c, proc_id);
    push16(&c, index);
    push16(&c, vrefnum);
    push32(&c, s);
    CALL(x, &c, &d0, NULL);
    *wdcb = rd32(x, s);
    return (int16_t)d0;
}

/* UTGetDefaultVol(wdpb): the default volume and folder in a WDParam block. */
static int ut_get_default_vol(ctx_t *x, uint32_t wdpb)
{
    uint32_t d0;
    trap_call_t c = ut(UT_GET_DEFAULT_VOL);

    push32(&c, wdpb);
    CALL(x, &c, &d0, NULL);
    return (int16_t)d0;
}

/* UTResolveFCB(refNum, &fcb) */
static int ut_resolve_fcb(ctx_t *x, uint32_t refnum, uint32_t *fcb)
{
    uint32_t s = slot(x), d0;
    trap_call_t c = ut(UT_RESOLVE_FCB);

    push16(&c, refnum);
    push32(&c, s);
    CALL(x, &c, &d0, NULL);
    *fcb = rd32(x, s);
    return (int16_t)d0;
}

static int ut_simple16(ctx_t *x, uint32_t selector, uint32_t value)
{
    uint32_t d0;
    trap_call_t c = ut(selector);

    push16(&c, value);
    CALL(x, &c, &d0, NULL);
    return (int16_t)d0;
}

static int ut_simple32(ctx_t *x, uint32_t selector, uint32_t value)
{
    uint32_t d0;
    trap_call_t c = ut(selector);

    push32(&c, value);
    CALL(x, &c, &d0, NULL);
    return (int16_t)d0;
}

/* ---- The host's items ---- */

static int get_item(ctx_t *x, uint32_t id, mac_fs_item_t *item)
{
    const mac_fs_ops_t *ops = x->v->ops;

    return ops->get(ops->ctx, id, item);
}

/* The folder a request starts from (its name is relative to it). */
static int start_dir(ctx_t *x, uint32_t pb, uint32_t dir_id, bool hide_name, uint32_t *dir)
{
    int16_t status;
    int r = ut_determine_vol(x, pb, hide_name, &status);

    if (r != NO_ERR)
        return r;

    switch (status)
    {
    case DTMV_FULL_PATHNAME:
        *dir = MAC_FS_ROOT_ID;
        return NO_ERR;
    case DTMV_VREFNUM:
    case DTMV_DRIVENUM:
        *dir = dir_id != 0 ? dir_id : MAC_FS_ROOT_ID;
        return NO_ERR;
    case DTMV_WDREFNUM:
        if (dir_id != 0)
        {
            *dir = dir_id;
            return NO_ERR;
        }
        else
        {
            uint32_t wdcb;

            r = ut_resolve_wdcb(x, 0, 0, rd16(x, pb + IO_VREFNUM), &wdcb);
            if (r != NO_ERR)
                return r;
            *dir = rd32(x, wdcb + WD_DIR_ID);
            return NO_ERR;
        }
    case DTMV_DEFAULT:
        if (dir_id != 0)
        {
            *dir = dir_id;
            return NO_ERR;
        }
        else
        {
            uint32_t wdpb = slot(x);

            if (x->f->next == x->f->count)
                wr32(x, wdpb + IO_NAME_PTR, 0);
            r = ut_get_default_vol(x, wdpb);
            if (r != NO_ERR)
                return r;
            *dir = rd32(x, wdpb + IO_WD_DIR_ID);
            return NO_ERR;
        }
    default:
        return PARAM_ERR;
    }
}

/*
 * Walk a pathname from folder dir: "a:b:c" from dir, ":a:b" from dir, "vol:a"
 * from the root (a full pathname), "::" one folder up, a trailing ':' names
 * the folder itself.
 */
static int walk(ctx_t *x, uint32_t dir, const uint8_t *name, uint32_t length, mac_fs_item_t *item)
{
    const mac_fs_ops_t *ops = x->v->ops;
    uint32_t i = 0;
    bool has_colon = false;
    mac_fs_item_t cur;

    for (uint32_t k = 0; k < length; k++)
        has_colon = has_colon || name[k] == ':';

    if (length > 0 && name[0] != ':' && has_colon)
    {
        /* A full pathname: skip the volume's name (the FSM chose the volume by it). */
        while (i < length && name[i] != ':')
            i++;
        i++;
        dir = MAC_FS_ROOT_ID;
    }
    else if (length > 0 && name[0] == ':')
    {
        i = 1;
    }
    if (dir == MAC_FS_ROOT_PARENT)
    {
        /* The root's parent (fsRtParID) holds one item: the root, called by the volume's name. */
        uint32_t j = i;

        while (j < length && name[j] != ':')
            j++;
        if (get_item(x, MAC_FS_ROOT_ID, &cur) != 0)
            return DIR_NF_ERR;
        if (j - i != cur.name[0])
            return j < length ? DIR_NF_ERR : FNF_ERR;
        for (uint32_t k = 0; k < j - i; k++)
        {
            uint8_t a = name[i + k], b = cur.name[1 + k];

            if ((a >= 'a' && a <= 'z' ? a - 32 : a) != (b >= 'a' && b <= 'z' ? b - 32 : b))
                return j < length ? DIR_NF_ERR : FNF_ERR;
        }
        if (j >= length - 1)
        {
            *item = cur;
            return NO_ERR;
        }
        i = j + 1;
        dir = MAC_FS_ROOT_ID;
    }
    if (get_item(x, dir, &cur) != 0 || !cur.is_dir)
        return DIR_NF_ERR;

    while (i < length)
    {
        uint32_t j = i;

        while (j < length && name[j] != ':')
            j++;
        if (j == i)
        {
            /* "::": the parent */
            if (cur.id == MAC_FS_ROOT_ID || get_item(x, cur.parent, &cur) != 0)
                return DIR_NF_ERR;
            i = j + 1;
            continue;
        }

        mac_fs_item_t next;

        if (j - i > MAC_FS_NAME_MAX)
            return BD_NAM_ERR;
        if (ops->lookup(ops->ctx, cur.id, name + i, j - i, &next) != 0)
            return j < length ? DIR_NF_ERR : FNF_ERR;
        if (j == length)
        {
            *item = next;
            return NO_ERR;
        }
        if (!next.is_dir)
            return DIR_NF_ERR;
        cur = next;
        i = j + 1;
    }
    *item = cur;
    return NO_ERR;
}

/* The item a request names (ioNamePtr relative to its volume/folder and dir_id). */
static int resolve(ctx_t *x, uint32_t pb, uint32_t dir_id, mac_fs_item_t *item)
{
    uint32_t dir;
    int r = start_dir(x, pb, dir_id, false, &dir);

    if (r != NO_ERR)
        return r;

    uint8_t name[256];
    uint32_t ptr = rd32(x, pb + IO_NAME_PTR);
    uint32_t length = ptr != 0 ? rd8(x, ptr) : 0;

    for (uint32_t i = 0; i < length; i++)
        name[i] = (uint8_t)rd8(x, ptr + 1 + i);
    return walk(x, dir, name, length, item);
}

static uint32_t physical(uint32_t length)
{
    return (length + BLOCK - 1) / BLOCK * BLOCK;
}

/*
 * Fill the file or folder fields of a CInfoPBRec, or of the HFileParam of
 * GetFileInfo; hfs false: the 80-byte FileParam of the MFS form, which ends
 * before the backup date.
 */
static void fill_info(ctx_t *x, uint32_t pb, const mac_fs_item_t *it, bool write_name, bool hfs)
{
    uint32_t ptr = rd32(x, pb + IO_NAME_PTR);

    if (write_name && ptr != 0)
        wr_pstring(x, ptr, it->name, 32);
    wr16(x, pb + IO_REFNUM, 0);
    wr8(x, pb + IO_FL_ATTRIB, it->is_dir ? 0x10 : (it->locked ? 0x01 : 0));     /* ioFlAttrib: bit 4 folder, bit 0 locked */
    wr8(x, pb + IO_ACUSER, 0);
    wr_bytes(x, pb + IO_FL_FNDR_INFO, it->finfo, 16);
    wr32(x, pb + IO_DIR_ID, it->id);
    if (it->is_dir)
    {
        wr16(x, pb + IO_DR_NM_FLS, it->valence > 0xFFFF ? 0xFFFF : it->valence);
    }
    else
    {
        wr16(x, pb + IO_FL_ST_BLK, 0);
        wr32(x, pb + IO_FL_LG_LEN, it->data_length);
        wr32(x, pb + IO_FL_PY_LEN, physical(it->data_length));
        wr16(x, pb + IO_FL_RST_BLK, 0);
        wr32(x, pb + IO_FL_RLG_LEN, it->rsrc_length);
        wr32(x, pb + IO_FL_RPY_LEN, physical(it->rsrc_length));
    }
    wr32(x, pb + IO_FL_CR_DAT, it->created);
    wr32(x, pb + IO_FL_MD_DAT, it->modified);
    if (!hfs)
        return;
    wr32(x, pb + IO_FL_BK_DAT, 0);
    wr_bytes(x, pb + IO_FL_XFNDR_INFO, it->fxinfo, 16);
    wr32(x, pb + IO_FL_PAR_ID, it->parent);
    if (!it->is_dir)
        wr32(x, pb + IO_FL_CLP_SIZ, 0);
}

/* ---- The volume ---- */

/* What the Mac is told about a volume's size and contents. */
typedef struct
{
    uint32_t block_size;        /* allocation blocks: at most 65535 of them cover the volume (up to 2 GB) */
    uint32_t blocks, free_blocks;
    uint32_t files, folders;
    bool locked;
} geometry_t;

static void geometry(ctx_t *x, geometry_t *g)
{
    const mac_fs_ops_t *ops = x->v->ops;
    mac_fs_volume_t info;
    mac_fs_item_t it;

    if (ops->volume == NULL || ops->volume(ops->ctx, &info) != 0)
    {
        /* Counted from the root folder: a read-only volume of unknown size. */
        g->block_size = BLOCK;
        g->blocks = 0xFFFF;
        g->free_blocks = 0;
        g->files = g->folders = 0;
        for (uint32_t i = 1; ops->child(ops->ctx, MAC_FS_ROOT_ID, i, &it) == 0; i++)
            (*(it.is_dir ? &g->folders : &g->files))++;
        g->locked = true;
        return;
    }

    /* System 7.5 knows volumes of up to 2 GB: a larger one is shown as 2 GB. */
    uint64_t total = info.total_bytes < 0x80000000u ? info.total_bytes : 0x80000000u;
    uint64_t free = info.free_bytes < total ? info.free_bytes : total;

    g->block_size = BLOCK;
    while (total / g->block_size > 0xFFFF)
        g->block_size *= 2;
    g->blocks = (uint32_t)(total / g->block_size);
    g->free_blocks = (uint32_t)(free / g->block_size);
    g->files = info.files;
    g->folders = info.folders;
    g->locked = info.read_only || ops->create == NULL;
}

/* After a change: the free space and counts in the VCB, which some code reads directly. */
static void refresh_vcb(ctx_t *x)
{
    geometry_t g;

    geometry(x, &g);
    wr16(x, x->v->vcb + VCB_FREE_BKS, g.free_blocks);
    wr32(x, x->v->vcb + VCB_FIL_CNT, g.files);
    wr32(x, x->v->vcb + VCB_DIR_CNT, g.folders);
}

static bool writable(ctx_t *x)
{
    geometry_t g;

    geometry(x, &g);
    return !g.locked;
}

/*
 * The next FCB of ours after fcb (0: the first) that is open on file id: its
 * data fork (fork 0), resource fork (1) or either (-1). 0 when there is none.
 */
static uint32_t next_fcb(ctx_t *x, uint32_t fcb, uint32_t id, int fork)
{
    uint32_t base = rd32(x, LM_FCBS_PTR), length = rd16(x, LM_FCB_LEN);

    if (base == 0 || length == 0)
        return 0;

    uint32_t end = base + rd16(x, base);

    for (fcb = fcb == 0 ? base + 2 : fcb + length; fcb + length <= end; fcb += length)
    {
        if (rd32(x, fcb + FCB_FL_NM) != id || rd32(x, fcb + FCB_VPTR) != x->v->vcb)
            continue;
        if (fork < 0 || ((rd8(x, fcb + FCB_FLAGS) & FCB_RESOURCE) != 0) == (fork == 1))
            return fcb;
    }
    return 0;
}

/* Every open FCB of a fork gets its new length (marks past it move back to it). */
static void set_fcb_eofs(ctx_t *x, uint32_t id, int fork, uint32_t eof)
{
    for (uint32_t fcb = next_fcb(x, 0, id, fork); fcb != 0; fcb = next_fcb(x, fcb, id, fork))
    {
        wr32(x, fcb + FCB_EOF, eof);
        wr32(x, fcb + FCB_PLEN, physical(eof));
        if (rd32(x, fcb + FCB_CRPS) > eof)
            wr32(x, fcb + FCB_CRPS, eof);
    }
}

/* ---- The HFS component's requests ---- */

static int do_get_cat_info(ctx_t *x, uint32_t pb)
{
    const mac_fs_ops_t *ops = x->v->ops;
    int16_t index = (int16_t)rd16(x, pb + IO_FDIR_INDEX);
    mac_fs_item_t it;
    int r;

    if (index < 0)
    {
        if (get_item(x, rd32(x, pb + IO_DIR_ID), &it) != 0 || !it.is_dir)
            return DIR_NF_ERR;
    }
    else if (index == 0)
    {
        r = resolve(x, pb, rd32(x, pb + IO_DIR_ID), &it);
        if (r != NO_ERR)
            return r;
    }
    else
    {
        uint32_t dir;

        r = start_dir(x, pb, rd32(x, pb + IO_DIR_ID), true, &dir);
        if (r != NO_ERR)
            return r;
        if (ops->child(ops->ctx, dir, (uint32_t)index, &it) != 0)
            return FNF_ERR;
    }
    fill_info(x, pb, &it, index != 0, true);
    return NO_ERR;
}

static int do_get_file_info(ctx_t *x, uint32_t pb, bool hfs)
{
    const mac_fs_ops_t *ops = x->v->ops;
    int16_t index = (int16_t)rd16(x, pb + IO_FDIR_INDEX);
    uint32_t dir_id = hfs ? rd32(x, pb + IO_DIR_ID) : 0;
    mac_fs_item_t it;
    int r;

    if (index <= 0)
    {
        r = resolve(x, pb, dir_id, &it);
        if (r != NO_ERR)
            return r;
        if (it.is_dir)
            return FNF_ERR;
    }
    else
    {
        /* By index: the files of the folder only (the MFS heritage of this call). */
        uint32_t dir, files = 0;

        r = start_dir(x, pb, dir_id, true, &dir);
        if (r != NO_ERR)
            return r;
        for (uint32_t i = 1;; i++)
        {
            if (ops->child(ops->ctx, dir, i, &it) != 0)
                return FNF_ERR;
            if (!it.is_dir && ++files == (uint32_t)index)
                break;
        }
    }

    fill_info(x, pb, &it, index > 0, hfs);
    return NO_ERR;
}

static int do_open(ctx_t *x, uint32_t pb, uint32_t vcb, bool hfs, bool resource_fork)
{
    mac_fs_item_t it;
    int r = resolve(x, pb, hfs ? rd32(x, pb + IO_DIR_ID) : 0, &it);

    if (r != NO_ERR)
        return r;
    if (it.is_dir)
        return FNF_ERR;

    /*
     * Permissions: fsCurPerm 0 (write if possible), fsRdPerm 1, fsWrPerm 2, fsRdWrPerm 3, fsRdWrShPerm
     * 4 (shared). On a read-only volume or a locked file a request to write gets permErr, as for a file
     * the user may not change: SimpleText then opens it again for reading (it gives up after wPrErr,
     * vLckdErr or wrPermErr; tried on System 7.5.3). One writer at a time (unless all share): another
     * gets opWrErr and the writer's reference number.
     */
    uint32_t permission = rd8(x, pb + IO_PERMSSN);
    bool can_write = writable(x) && !it.locked;
    bool write = permission >= 2 || (permission == 0 && can_write);
    bool shared = permission == 4;

    if (write && !can_write)
        return PERM_ERR;
    for (uint32_t other = next_fcb(x, 0, it.id, resource_fork ? 1 : 0); write && other != 0;
         other = next_fcb(x, other, it.id, resource_fork ? 1 : 0))
    {
        uint32_t flags = rd8(x, other + FCB_FLAGS);

        if (!(flags & FCB_WRITE) || (shared && (flags & FCB_SHARED_WRITE)))
            continue;
        if (permission == 0)
        {
            write = false;      /* fsCurPerm: read only, then */
            break;
        }
        wr16(x, pb + IO_REFNUM, other - rd32(x, LM_FCBS_PTR));
        return OP_WR_ERR;
    }

    uint32_t s = slot(x), d0;
    trap_call_t c = ut(UT_ALLOCATE_FCB);

    push32(&c, pb + IO_REFNUM);
    push32(&c, s);
    CALL(x, &c, &d0, NULL);
    if ((int16_t)d0 != NO_ERR)
        return (int16_t)d0;

    uint32_t fcb = rd32(x, s);
    uint32_t length = resource_fork ? it.rsrc_length : it.data_length;

    wr_zero(x, fcb, FCB_SIZE);
    wr32(x, fcb + FCB_FL_NM, it.id);
    wr8(x, fcb + FCB_FLAGS, (resource_fork ? FCB_RESOURCE : 0) | (write ? FCB_WRITE : 0) |
                                (write && shared ? FCB_SHARED_WRITE : 0) | (can_write ? 0 : FCB_FILE_LOCKED));
    wr32(x, fcb + FCB_EOF, length);
    wr32(x, fcb + FCB_PLEN, physical(length));
    wr32(x, fcb + FCB_CRPS, 0);
    wr32(x, fcb + FCB_VPTR, vcb);
    wr32(x, fcb + FCB_CLMP_SIZE, BLOCK);
    wr_bytes(x, fcb + FCB_FTYPE, it.finfo, 4);
    wr32(x, fcb + FCB_DIR_ID, it.parent);
    wr_pstring(x, fcb + FCB_CNAME, it.name, 32);
    return NO_ERR;
}

/* The FCB of an open file of ours. */
static int open_fcb(ctx_t *x, uint32_t pb, uint32_t *fcb)
{
    int r = ut_resolve_fcb(x, rd16(x, pb + IO_REFNUM), fcb);

    if (r != NO_ERR)
        return r;
    if (rd32(x, *fcb + FCB_FL_NM) == 0)
        return FN_OPN_ERR;
    return NO_ERR;
}

/* The position ioPosMode/ioPosOffset ask for; -1 if it is before the start. */
static int64_t position(ctx_t *x, uint32_t pb, uint32_t fcb)
{
    int32_t offset = (int32_t)rd32(x, pb + IO_POS_OFFSET);
    int64_t mark = rd32(x, fcb + FCB_CRPS);
    int64_t eof = rd32(x, fcb + FCB_EOF);
    int64_t pos;

    switch (rd16(x, pb + IO_POS_MODE) & 3)
    {
    case 1:
        pos = offset;
        break;
    case 2:
        pos = eof + offset;
        break;
    case 3:
        pos = mark + offset;
        break;
    default:
        pos = mark;
        break;
    }
    return pos < 0 ? -1 : pos;
}

static int do_read(ctx_t *x, uint32_t pb)
{
    const mac_fs_ops_t *ops = x->v->ops;
    uint32_t fcb;
    int r = open_fcb(x, pb, &fcb);

    if (r != NO_ERR)
        return r;
    if ((int32_t)rd32(x, pb + IO_REQ_COUNT) < 0)
        return PARAM_ERR;

    int64_t pos = position(x, pb, fcb);
    uint32_t eof = rd32(x, fcb + FCB_EOF);

    wr32(x, pb + IO_ACT_COUNT, 0);
    if (pos < 0)
        return POS_ERR;
    if (pos > eof)
        pos = eof;

    uint32_t want = rd32(x, pb + IO_REQ_COUNT);
    uint32_t have = eof - (uint32_t)pos;
    uint32_t count = want < have ? want : have;
    uint32_t buffer = rd32(x, pb + IO_BUFFER);
    uint32_t mode = rd16(x, pb + IO_POS_MODE);
    bool newline = (mode & 0x80) != 0;
    uint8_t newline_char = (uint8_t)(mode >> 8);
    uint32_t id = rd32(x, fcb + FCB_FL_NM);
    bool rsrc = (rd8(x, fcb + FCB_FLAGS) & FCB_RESOURCE) != 0;
    uint32_t done = 0;
    bool stopped = false;
    uint8_t chunk[512];

    while (done < count && !stopped)
    {
        uint32_t n = count - done < sizeof chunk ? count - done : sizeof chunk;
        int32_t got = ops->read(ops->ctx, id, rsrc, (uint32_t)pos + done, chunk, n);

        if (got <= 0)
            break;
        for (int32_t i = 0; i < got; i++)
        {
            wr8(x, buffer + done, chunk[i]);
            done++;
            if (newline && chunk[i] == newline_char)
            {
                stopped = true;
                break;
            }
        }
        if ((uint32_t)got < n)
            break;
    }
    wr32(x, pb + IO_ACT_COUNT, done);
    wr32(x, fcb + FCB_CRPS, (uint32_t)pos + done);
    wr32(x, pb + IO_POS_OFFSET, (uint32_t)pos + done);
    return stopped || done == want ? NO_ERR : EOF_ERR;
}

static int do_set_fpos(ctx_t *x, uint32_t pb)
{
    uint32_t fcb;
    int r = open_fcb(x, pb, &fcb);

    if (r != NO_ERR)
        return r;

    int64_t pos = position(x, pb, fcb);
    uint32_t eof = rd32(x, fcb + FCB_EOF);

    if (pos < 0)
        return POS_ERR;
    r = NO_ERR;
    if (pos > eof)
    {
        pos = eof;
        r = EOF_ERR;
    }
    wr32(x, fcb + FCB_CRPS, (uint32_t)pos);
    wr32(x, pb + IO_POS_OFFSET, (uint32_t)pos);
    return r;
}

static int do_get_fcb_info(ctx_t *x, uint32_t pb, uint32_t vcb)
{
    uint32_t fcb = 0;
    int16_t index = (int16_t)rd16(x, pb + IO_FCB_INDX);
    int r;

    if (index == 0)
    {
        r = ut_resolve_fcb(x, rd16(x, pb + IO_REFNUM), &fcb);
        if (r != NO_ERR)
            return r;
    }
    else
    {
        /* UTIndexFCB walks the FCBs of a volume; its refNum argument is the iterator. */
        uint32_t iter = x->fs->data + DATA_SCRATCH + x->depth * MAC_FSM_CALLS * SLOT + (MAC_FSM_CALLS - 1) * SLOT;

        if (x->f->count == 0)
            wr16(x, iter, 0);
        for (int i = 0; i < index; i++)
        {
            uint32_t s = slot(x), d0;
            trap_call_t c = ut(UT_INDEX_FCB);

            push32(&c, vcb);
            push32(&c, iter);
            push32(&c, s);
            CALL(x, &c, &d0, NULL);
            if ((int16_t)d0 != NO_ERR)
                return (int16_t)d0;
            fcb = rd32(x, s);
        }
        wr16(x, pb + IO_REFNUM, rd16(x, iter));
    }
    if (fcb == 0)
        return RF_NUM_ERR;

    uint32_t ptr = rd32(x, pb + IO_NAME_PTR);

    if (ptr != 0)
    {
        for (uint32_t i = 0; i < 32; i++)
            wr8(x, ptr + i, rd8(x, fcb + FCB_CNAME + i));
    }
    wr32(x, pb + IO_FCB_FL_NM, rd32(x, fcb + FCB_FL_NM));
    wr8(x, pb + IO_FCB_FLAGS, rd8(x, fcb + FCB_FLAGS));
    wr16(x, pb + IO_FCB_ST_BLK, 0);
    wr32(x, pb + IO_FCB_EOF, rd32(x, fcb + FCB_EOF));
    wr32(x, pb + IO_FCB_PLEN, rd32(x, fcb + FCB_PLEN));
    wr32(x, pb + IO_FCB_CRPS, rd32(x, fcb + FCB_CRPS));
    wr16(x, pb + IO_FCB_VREFNUM, rd16(x, rd32(x, fcb + FCB_VPTR) + VCB_VREFNUM));
    wr32(x, pb + IO_FCB_CLP_SIZ, rd32(x, fcb + FCB_CLMP_SIZE));
    wr32(x, pb + IO_FCB_PAR_ID, rd32(x, fcb + FCB_DIR_ID));
    return NO_ERR;
}

static int do_get_vol_info(ctx_t *x, uint32_t pb, uint32_t vcb, bool hfs)
{
    mac_fs_item_t root;
    geometry_t g;
    uint32_t ptr = rd32(x, pb + IO_NAME_PTR);

    if (get_item(x, MAC_FS_ROOT_ID, &root) != 0)
        return NSV_ERR;
    geometry(x, &g);
    if (ptr != 0)
    {
        for (uint32_t i = 0; i < 28; i++)
            wr8(x, ptr + i, rd8(x, vcb + VCB_VN + i));
    }
    wr16(x, pb + IO_VREFNUM, rd16(x, vcb + VCB_VREFNUM));
    wr32(x, pb + IO_VCR_DATE, root.created);
    wr32(x, pb + IO_VLS_MOD, root.modified);
    wr16(x, pb + IO_VATRB, g.locked ? VA_HARD_LOCK : 0);
    wr16(x, pb + IO_VNM_FLS, rd16(x, vcb + VCB_NM_FLS));
    wr16(x, pb + 42, 0);        /* ioVBitMap */
    wr16(x, pb + 44, 0);        /* ioAllocPtr */
    wr16(x, pb + IO_VNM_AL_BLKS, g.blocks);
    wr32(x, pb + IO_VAL_BLK_SIZ, g.block_size);
    wr32(x, pb + IO_VCLP_SIZ, g.block_size);
    wr16(x, pb + 56, 0);        /* ioAlBlSt */
    wr32(x, pb + IO_VNXT_CNID, 0x7FFFFFFF);
    wr16(x, pb + IO_VFR_BLK, g.free_blocks);
    if (hfs)
    {
        wr16(x, pb + IO_VSIG_WORD, 0x4244);
        wr16(x, pb + IO_VDRV_INFO, (uint16_t)x->v->drive);
        wr16(x, pb + IO_VDREFNUM, (uint16_t)DISK_REFNUM);
        wr16(x, pb + IO_VFSID, FSID);
        wr32(x, pb + 72, 0);    /* ioVBkUp */
        wr16(x, pb + 76, 0);    /* ioVSeqNum */
        wr32(x, pb + 78, 0);    /* ioVWrCnt */
        wr32(x, pb + IO_VFIL_CNT, g.files);
        wr32(x, pb + IO_VDIR_CNT, g.folders);
        wr_zero(x, pb + IO_VFNDR_INFO, 32);
    }
    return NO_ERR;
}

static int do_get_vol_parms(ctx_t *x, uint32_t pb)
{
    static const uint8_t info[20] = {
        0x00, 0x02,                         /* vMVersion 2 */
        (uint8_t)(GVP_ATTRIB >> 24), (uint8_t)(GVP_ATTRIB >> 16), (uint8_t)(GVP_ATTRIB >> 8), (uint8_t)GVP_ATTRIB,
    };
    uint32_t count = rd32(x, pb + IO_REQ_COUNT);

    if (count > sizeof info)
        count = sizeof info;
    wr_bytes(x, rd32(x, pb + IO_BUFFER), info, count);
    wr32(x, pb + IO_ACT_COUNT, count);
    return NO_ERR;
}

static int do_set_vol(ctx_t *x, uint32_t pb, uint32_t vcb, bool hfs)
{
    uint32_t dir_id, refnum, d0;
    int r;

    if (hfs)
    {
        mac_fs_item_t it;

        r = resolve(x, pb, rd32(x, pb + IO_WD_DIR_ID), &it);
        if (r != NO_ERR)
            return r;
        if (!it.is_dir)
            return DIR_NF_ERR;
        dir_id = it.id;
        refnum = rd16(x, vcb + VCB_VREFNUM);
    }
    else
    {
        r = ut_simple16(x, UT_CHECK_WDREFNUM, rd16(x, pb + IO_VREFNUM));
        if (r == PENDING)
            return PENDING;
        if (r != NO_ERR)
        {
            dir_id = MAC_FS_ROOT_ID;     /* a volume reference number */
            refnum = rd16(x, vcb + VCB_VREFNUM);
        }
        else
        {
            dir_id = 0;                  /* a working directory */
            refnum = rd16(x, pb + IO_VREFNUM);
        }
    }

    trap_call_t c = ut(UT_SET_DEFAULT_VOL);

    push32(&c, 0);
    push32(&c, dir_id);
    push16(&c, refnum);
    CALL(x, &c, &d0, NULL);
    return (int16_t)d0;
}

static int do_get_wd_info(ctx_t *x, uint32_t pb, uint32_t vcb)
{
    uint32_t ptr = rd32(x, pb + IO_NAME_PTR);

    if (rd16(x, pb + IO_WD_INDEX) == 0 && rd16(x, pb + IO_VREFNUM) == rd16(x, vcb + VCB_VREFNUM))
    {
        wr32(x, pb + IO_WD_PROC_ID, 0);
        wr16(x, pb + IO_WD_VREFNUM, rd16(x, vcb + VCB_VREFNUM));
        if (ptr != 0)
        {
            for (uint32_t i = 0; i < 28; i++)
                wr8(x, ptr + i, rd8(x, vcb + VCB_VN + i));
        }
        wr32(x, pb + IO_WD_DIR_ID, MAC_FS_ROOT_ID);
        return NO_ERR;
    }

    uint32_t wdcb;
    int r = ut_resolve_wdcb(x, rd32(x, pb + IO_WD_PROC_ID), rd16(x, pb + IO_WD_INDEX), rd16(x, pb + IO_VREFNUM),
                            &wdcb);

    if (r != NO_ERR)
        return r;

    uint32_t wvcb = rd32(x, wdcb + WD_VCB_PTR);

    wr32(x, pb + IO_WD_PROC_ID, rd32(x, wdcb + WD_PROC_ID));
    wr16(x, pb + IO_WD_VREFNUM, rd16(x, wvcb + VCB_VREFNUM));
    if (ptr != 0)
    {
        for (uint32_t i = 0; i < 28; i++)
            wr8(x, ptr + i, rd8(x, wvcb + VCB_VN + i));
    }
    wr32(x, pb + IO_WD_DIR_ID, rd32(x, wdcb + WD_DIR_ID));
    return NO_ERR;
}

static int do_volume_mount(ctx_t *x, uint32_t pb)
{
    /* Which of our volumes: the index the install put after the VolumeMountInfoHeader. */
    uint32_t index = rd16(x, rd32(x, pb + IO_BUFFER) + 6);

    if (index >= x->fs->volumes)
        return PARAM_ERR;

    mac_fsm_volume_t *v = &x->fs->volume[index];

    x->v = v;

    uint32_t s = slot(x), d0;
    trap_call_t c = ut(UT_ALLOCATE_VCB);

    /* UTAllocateVCB(&sysVCBLength, &vcb, addSize) */
    push32(&c, s);
    push32(&c, s + 2);
    push16(&c, 0);
    CALL(x, &c, &d0, NULL);
    if ((int16_t)d0 != NO_ERR)
        return (int16_t)d0;

    uint32_t vcb = rd32(x, s + 2);
    mac_fs_item_t root, child;
    geometry_t g;
    const char *name = v->ops->volume_name;
    uint32_t n = 0, root_files = 0;

    if (get_item(x, MAC_FS_ROOT_ID, &root) != 0)
        return NSV_ERR;
    geometry(x, &g);
    for (uint32_t i = 1; v->ops->child(v->ops->ctx, MAC_FS_ROOT_ID, i, &child) == 0; i++)
        root_files += child.is_dir ? 0 : 1;
    while (name[n] != '\0' && n < 27)
        n++;
    wr16(x, vcb + VCB_SIG_WORD, 0x4244);
    wr32(x, vcb + VCB_CR_DATE, root.created);
    wr32(x, vcb + VCB_LS_MOD, root.modified);
    wr16(x, vcb + VCB_ATRB, g.locked ? VA_HARD_LOCK : 0);
    wr16(x, vcb + VCB_NM_FLS, root_files > 0xFFFF ? 0xFFFF : root_files);
    wr16(x, vcb + VCB_NM_AL_BLKS, g.blocks);
    wr32(x, vcb + VCB_AL_BLK_SIZ, g.block_size);
    wr32(x, vcb + VCB_CLP_SIZ, g.block_size);
    wr32(x, vcb + VCB_NXT_CNID, 0x7FFFFFFF);
    wr16(x, vcb + VCB_FREE_BKS, g.free_blocks);
    wr8(x, vcb + VCB_VN, n);
    wr_bytes(x, vcb + VCB_VN + 1, (const uint8_t *)name, n);
    wr16(x, vcb + VCB_FSID, FSID);
    wr32(x, vcb + VCB_FIL_CNT, g.files);
    wr32(x, vcb + VCB_DIR_CNT, g.folders);

    /* UTAddNewVCB(drvNum, &vRefNum, vcb) */
    c = ut(UT_ADD_NEW_VCB);
    s = slot(x);
    push16(&c, (uint16_t)v->drive);
    push32(&c, s);
    push32(&c, vcb);
    CALL(x, &c, &d0, NULL);
    if ((int16_t)d0 != NO_ERR)
        return (int16_t)d0;

    /* A disk-inserted event: the Finder shows the volume (PostEvent: A0 = event, D0 = message). */
    c = reg_trap(TRAP_POST_EVENT);
    c.a[0] = 7;
    c.d[0] = (uint32_t)v->drive;
    CALL(x, &c, &d0, NULL);

    v->vcb = vcb;
    v->vrefnum = (int16_t)rd16(x, vcb + VCB_VREFNUM);
    v->mounted = true;
    wr16(x, pb + IO_VREFNUM, (uint16_t)v->vrefnum);
    return NO_ERR;
}

/* ---- Changing the volume ---- */

/* A Pascal string at ptr in Mac memory; its length (0 for no string). */
static uint32_t rd_pstring(ctx_t *x, uint32_t ptr, uint8_t *out, uint32_t size)
{
    uint32_t n = ptr != 0 ? rd8(x, ptr) : 0;

    if (n > size)
        n = size;
    for (uint32_t i = 0; i < n; i++)
        out[i] = (uint8_t)rd8(x, ptr + 1 + i);
    return n;
}

/*
 * The folder a new item goes into and the item's name: the last part of the
 * request's pathname (ioNamePtr relative to its volume/folder and dir_id).
 */
static int resolve_parent(ctx_t *x, uint32_t pb, uint32_t dir_id, uint32_t *parent, uint8_t *leaf, uint32_t *length)
{
    uint32_t dir;
    int r = start_dir(x, pb, dir_id, false, &dir);

    if (r != NO_ERR)
        return r;

    uint8_t name[256];
    uint32_t n = rd_pstring(x, rd32(x, pb + IO_NAME_PTR), name, 255);
    uint32_t cut = n;

    while (cut > 0 && name[cut - 1] != ':')
        cut--;
    if (n - cut == 0 || n - cut > MAC_FS_NAME_MAX)
        return BD_NAM_ERR;
    *length = n - cut;
    k_memcpy(leaf, name + cut, *length);
    if (cut == 0)
    {
        *parent = dir;
        return NO_ERR;
    }

    mac_fs_item_t folder;

    r = walk(x, dir, name, cut, &folder);
    if (r == FNF_ERR)
        r = DIR_NF_ERR;
    if (r != NO_ERR)
        return r;
    if (!folder.is_dir)
        return DIR_NF_ERR;
    *parent = folder.id;
    return NO_ERR;
}

static int do_create(ctx_t *x, uint32_t pb, bool hfs, bool is_dir)
{
    const mac_fs_ops_t *ops = x->v->ops;
    uint32_t parent, length;
    uint8_t leaf[MAC_FS_NAME_MAX];
    mac_fs_item_t it;

    if (!writable(x))
        return WPR_ERR;

    int r = resolve_parent(x, pb, hfs ? rd32(x, pb + IO_DIR_ID) : 0, &parent, leaf, &length);

    if (r != NO_ERR)
        return r;
    r = ops->create(ops->ctx, parent, leaf, length, is_dir, &it);
    if (r != NO_ERR)
        return r;
    if (is_dir)
        wr32(x, pb + IO_DIR_ID, it.id);
    refresh_vcb(x);
    return NO_ERR;
}

static int do_delete(ctx_t *x, uint32_t pb, bool hfs)
{
    const mac_fs_ops_t *ops = x->v->ops;
    mac_fs_item_t it;

    if (!writable(x))
        return WPR_ERR;

    int r = resolve(x, pb, hfs ? rd32(x, pb + IO_DIR_ID) : 0, &it);

    if (r != NO_ERR)
        return r;
    if (it.id == MAC_FS_ROOT_ID || (it.is_dir && it.valence != 0) || (!it.is_dir && next_fcb(x, 0, it.id, -1) != 0))
        return FBSY_ERR;
    if (it.locked)
        return FLCKD_ERR;
    r = ops->remove(ops->ctx, it.id);
    if (r == NO_ERR)
        refresh_vcb(x);
    return r;
}

static int do_rename(ctx_t *x, uint32_t pb, bool hfs)
{
    const mac_fs_ops_t *ops = x->v->ops;
    mac_fs_item_t it;
    uint8_t name[MAC_FS_NAME_MAX + 1];

    if (!writable(x))
        return WPR_ERR;

    int r = resolve(x, pb, hfs ? rd32(x, pb + IO_DIR_ID) : 0, &it);

    if (r != NO_ERR)
        return r;
    if (it.id == MAC_FS_ROOT_ID)
        return PARAM_ERR;           /* the volume's name stays (GetVolParms says kNoVNEdit) */

    uint32_t ptr = rd32(x, pb + IO_MISC);
    uint32_t n = ptr != 0 ? rd8(x, ptr) : 0;

    if (n == 0 || n > MAC_FS_NAME_MAX)
        return BD_NAM_ERR;
    rd_pstring(x, ptr, name + 1, MAC_FS_NAME_MAX);
    name[0] = (uint8_t)n;
    for (uint32_t i = 1; i <= n; i++)
        if (name[i] == ':')
            return BD_NAM_ERR;
    if (it.locked)
        return FLCKD_ERR;
    r = ops->move(ops->ctx, it.id, it.parent, name + 1, n);
    if (r != NO_ERR)
        return r;
    for (uint32_t fcb = next_fcb(x, 0, it.id, -1); fcb != 0; fcb = next_fcb(x, fcb, it.id, -1))
        wr_pstring(x, fcb + FCB_CNAME, name, 32);
    return NO_ERR;
}

static int do_cat_move(ctx_t *x, uint32_t pb)
{
    const mac_fs_ops_t *ops = x->v->ops;
    mac_fs_item_t it, to;

    if (!writable(x))
        return WPR_ERR;

    int r = resolve(x, pb, rd32(x, pb + IO_DIR_ID), &it);

    if (r != NO_ERR)
        return r;
    if (it.id == MAC_FS_ROOT_ID)
        return BAD_MOV_ERR;

    /* The folder to move into: ioNewName relative to ioNewDirID (the root when it is 0). */
    uint8_t name[256];
    uint32_t to_dir = rd32(x, pb + IO_NEW_DIR_ID);
    uint32_t n = rd_pstring(x, rd32(x, pb + IO_NEW_NAME), name, 255);

    r = walk(x, to_dir != 0 ? to_dir : MAC_FS_ROOT_ID, name, n, &to);
    if (r != NO_ERR || !to.is_dir)
        return DIR_NF_ERR;
    r = ops->move(ops->ctx, it.id, to.id, NULL, 0);
    if (r != NO_ERR)
        return r;
    for (uint32_t fcb = next_fcb(x, 0, it.id, -1); fcb != 0; fcb = next_fcb(x, fcb, it.id, -1))
        wr32(x, fcb + FCB_DIR_ID, to.id);
    return NO_ERR;
}

/* SetCatInfo (all), SetFileInfo (finder info and two dates), SetFilLock/RstFilLock (lock 1/0). */
enum
{
    SET_CAT_INFO,
    SET_FILE_INFO,
    SET_LOCK,
    CLEAR_LOCK
};

static int do_set_info(ctx_t *x, uint32_t pb, bool hfs, int what)
{
    const mac_fs_ops_t *ops = x->v->ops;
    mac_fs_item_t it;

    if (!writable(x))
        return WPR_ERR;

    int r = resolve(x, pb, hfs || what == SET_CAT_INFO ? rd32(x, pb + IO_DIR_ID) : 0, &it);

    if (r != NO_ERR)
        return r;
    switch (what)
    {
    case SET_CAT_INFO:
        for (uint32_t i = 0; i < 16; i++)
        {
            it.finfo[i] = (uint8_t)rd8(x, pb + IO_FL_FNDR_INFO + i);
            it.fxinfo[i] = (uint8_t)rd8(x, pb + IO_FL_XFNDR_INFO + i);
        }
        it.created = rd32(x, pb + IO_FL_CR_DAT);
        it.modified = rd32(x, pb + IO_FL_MD_DAT);
        it.backed_up = rd32(x, pb + IO_FL_BK_DAT);
        break;
    case SET_FILE_INFO:
        if (it.is_dir)
            return FNF_ERR;
        for (uint32_t i = 0; i < 16; i++)
            it.finfo[i] = (uint8_t)rd8(x, pb + IO_FL_FNDR_INFO + i);
        it.created = rd32(x, pb + IO_FL_CR_DAT);
        it.modified = rd32(x, pb + IO_FL_MD_DAT);
        break;
    default:
        if (it.is_dir)
            return FNF_ERR;
        it.locked = what == SET_LOCK;
        break;
    }
    return ops->set_info(ops->ctx, &it);
}

static int do_write(ctx_t *x, uint32_t pb)
{
    static uint8_t chunk[COPY_CHUNK];
    const mac_fs_ops_t *ops = x->v->ops;
    uint32_t fcb;
    int r = open_fcb(x, pb, &fcb);

    if (r != NO_ERR)
        return r;
    wr32(x, pb + IO_ACT_COUNT, 0);
    if (!(rd8(x, fcb + FCB_FLAGS) & FCB_WRITE))
        return WR_PERM_ERR;
    if ((int32_t)rd32(x, pb + IO_REQ_COUNT) < 0)
        return PARAM_ERR;

    int64_t pos = position(x, pb, fcb);
    uint32_t want = rd32(x, pb + IO_REQ_COUNT);

    if (pos < 0)
        return POS_ERR;
    if ((uint64_t)pos + want > 0x7FFFFFFFu)
        return DSK_FUL_ERR;     /* past the largest file the Mac knows */

    uint32_t buffer = rd32(x, pb + IO_BUFFER);
    uint32_t id = rd32(x, fcb + FCB_FL_NM);
    bool rsrc = (rd8(x, fcb + FCB_FLAGS) & FCB_RESOURCE) != 0;
    uint32_t done = 0;

    r = NO_ERR;
    while (done < want)
    {
        uint32_t n = want - done < COPY_CHUNK ? want - done : COPY_CHUNK;

        for (uint32_t i = 0; i < n; i++)
            chunk[i] = (uint8_t)rd8(x, buffer + done + i);

        int32_t put = ops->write(ops->ctx, id, rsrc, (uint32_t)pos + done, chunk, n);

        if (put < 0)
        {
            r = put;
            break;
        }
        done += (uint32_t)put;
        if ((uint32_t)put < n)
        {
            r = DSK_FUL_ERR;
            break;
        }
    }

    uint32_t end = (uint32_t)pos + done;

    if (end > rd32(x, fcb + FCB_EOF))
        set_fcb_eofs(x, id, rsrc ? 1 : 0, end);
    wr8(x, fcb + FCB_FLAGS, rd8(x, fcb + FCB_FLAGS) | FCB_MODIFIED);
    wr32(x, pb + IO_ACT_COUNT, done);
    wr32(x, fcb + FCB_CRPS, end);
    wr32(x, pb + IO_POS_OFFSET, end);
    refresh_vcb(x);
    return r;
}

static int do_set_eof(ctx_t *x, uint32_t pb)
{
    const mac_fs_ops_t *ops = x->v->ops;
    uint32_t fcb;
    int r = open_fcb(x, pb, &fcb);

    if (r != NO_ERR)
        return r;
    if (!(rd8(x, fcb + FCB_FLAGS) & FCB_WRITE))
        return WR_PERM_ERR;

    uint32_t eof = rd32(x, pb + IO_MISC);
    uint32_t id = rd32(x, fcb + FCB_FL_NM);
    bool rsrc = (rd8(x, fcb + FCB_FLAGS) & FCB_RESOURCE) != 0;

    if ((int32_t)eof < 0)
        return PARAM_ERR;
    r = ops->set_length(ops->ctx, id, rsrc, eof);
    if (r != NO_ERR)
        return r;
    set_fcb_eofs(x, id, rsrc ? 1 : 0, eof);
    refresh_vcb(x);
    return NO_ERR;
}

/* Allocate: space is taken as it is written, so the request is granted as it is. */
static int do_allocate(ctx_t *x, uint32_t pb)
{
    uint32_t fcb;
    int r = open_fcb(x, pb, &fcb);

    if (r != NO_ERR)
        return r;
    if (!(rd8(x, fcb + FCB_FLAGS) & FCB_WRITE))
        return WR_PERM_ERR;
    wr32(x, pb + IO_ACT_COUNT, rd32(x, pb + IO_REQ_COUNT));
    return NO_ERR;
}

static int hfs_request(ctx_t *x)
{
    mac_fsm_frame_t *f = x->f;
    uint32_t pb = f->pb, vcb = f->vcb;
    bool hfs = (f->select & HFS_BIT) != 0;
    uint32_t fcb;
    int r;

    if ((f->select & 0xF0FF) == SEL_VOLUME_MOUNT)
        return do_volume_mount(x, pb);
    if ((f->select & 0xF0FF) == SEL_MOUNT_VOL)
    {
        for (uint32_t i = 0; i < x->fs->volumes; i++)
            if ((int16_t)rd16(x, pb + IO_VREFNUM) == x->fs->volume[i].drive)
                return NO_ERR;
        return EXT_FS_ERR;
    }
    if (x->v == NULL)
        return NSV_ERR;         /* not one of our volumes */

    switch (f->select & 0xF0FF)
    {
    case SEL_UNMOUNT_VOL:
        r = ut_simple32(x, UT_DISPOSE_VCB, vcb);
        if (r == NO_ERR)
            x->v->mounted = false;
        return r;
    case SEL_EJECT:
    case SEL_OFFLINE:
    case SEL_FLUSH_VOL:
    case SEL_FLUSH_FILE:
        return NO_ERR;
    case SEL_GET_VOL_INFO:
        return do_get_vol_info(x, pb, vcb, hfs);
    case SEL_GET_VOL_PARMS:
        return do_get_vol_parms(x, pb);
    case SEL_GET_VOL:
        return ut_simple32(x, UT_GET_DEFAULT_VOL, pb);
    case SEL_SET_VOL:
        return do_set_vol(x, pb, vcb, hfs);
    case SEL_GET_CAT_INFO:
        return do_get_cat_info(x, pb);
    case SEL_GET_FILE_INFO:
        return do_get_file_info(x, pb, hfs);
    case SEL_OPEN:
    case SEL_OPEN_DF:
        return do_open(x, pb, vcb, hfs || (f->select & 0xF0FF) == SEL_OPEN_DF, false);
    case SEL_OPEN_RF:
        return do_open(x, pb, vcb, hfs, true);
    case SEL_CLOSE:
        return ut_simple16(x, UT_RELEASE_FCB, rd16(x, pb + IO_REFNUM));
    case SEL_READ:
        return do_read(x, pb);
    case SEL_GET_EOF:
        r = open_fcb(x, pb, &fcb);
        if (r == NO_ERR)
            wr32(x, pb + IO_MISC, rd32(x, fcb + FCB_EOF));
        return r;
    case SEL_GET_FPOS:
        r = open_fcb(x, pb, &fcb);
        if (r == NO_ERR)
        {
            wr32(x, pb + IO_REQ_COUNT, 0);
            wr32(x, pb + IO_ACT_COUNT, 0);
            wr16(x, pb + IO_POS_MODE, 0);
            wr32(x, pb + IO_POS_OFFSET, rd32(x, fcb + FCB_CRPS));
        }
        return r;
    case SEL_SET_FPOS:
        return do_set_fpos(x, pb);
    case SEL_GET_FCB_INFO:
        return do_get_fcb_info(x, pb, vcb);
    case SEL_OPEN_WD:
        return ut_simple32(x, UT_ALLOCATE_WDCB, pb);
    case SEL_CLOSE_WD:
        return ut_simple16(x, UT_RELEASE_WDCB, rd16(x, pb + IO_VREFNUM));
    case SEL_GET_WD_INFO:
        return do_get_wd_info(x, pb, vcb);
    case SEL_WRITE:
        return do_write(x, pb);
    case SEL_ALLOCATE:
        return do_allocate(x, pb);
    case SEL_SET_EOF:
        return do_set_eof(x, pb);
    case SEL_CREATE:
        return do_create(x, pb, hfs, false);
    case SEL_DIR_CREATE:
        return do_create(x, pb, true, true);
    case SEL_DELETE:
        return do_delete(x, pb, hfs);
    case SEL_RENAME:
        return do_rename(x, pb, hfs);
    case SEL_CAT_MOVE:
        return do_cat_move(x, pb);
    case SEL_SET_FILE_INFO:
        return do_set_info(x, pb, hfs, SET_FILE_INFO);
    case SEL_SET_CAT_INFO:
        return do_set_info(x, pb, true, SET_CAT_INFO);
    case SEL_SET_FIL_LOCK:
        return do_set_info(x, pb, hfs, SET_LOCK);
    case SEL_RST_FIL_LOCK:
        return do_set_info(x, pb, hfs, CLEAR_LOCK);
    case SEL_SET_VOL_INFO:
        return writable(x) ? NO_ERR : WPR_ERR;  /* the volume's dates and Finder info are not kept */
    case SEL_LOCK_RANGE:
    case SEL_UNLOCK_RANGE:
        return open_fcb(x, pb, &fcb);           /* one user: ranges need no locks */
    case SEL_SET_FIL_TYPE:
        return writable(x) ? PARAM_ERR : WPR_ERR;
    default:
        return PARAM_ERR;
    }
}

/* ---- The install (registers the file system, adds its drive, mounts the volume) ---- */

static uint32_t ostype(const char *s)
{
    return ((uint32_t)(uint8_t)s[0] << 24) | ((uint32_t)(uint8_t)s[1] << 16) | ((uint32_t)(uint8_t)s[2] << 8) |
           (uint8_t)s[3];
}

/* A drive number no drive in the queue uses (above the .Disk drives). */
static int16_t free_drive(ctx_t *x)
{
    int16_t highest = MAC_FIRST_DRIVE + MAC_MAX_DISKS - 1;
    uint32_t el = rd32(x, LM_DRVQ_HDR + 2);

    for (int i = 0; el != 0 && i < 64; i++)
    {
        int16_t d = (int16_t)rd16(x, el + DRVQ_DRIVE);

        if (d > highest)
            highest = d;
        el = rd32(x, el);
    }
    return (int16_t)(highest + 1);
}

static int install(ctx_t *x)
{
    mac_fsm_t *fs = x->fs;
    uint32_t d0, a0;
    trap_call_t c;

    /* The File System Manager, version 1.2 or later (Gestalt 'fs  ' bit 2, 'fsm '). */
    c = reg_trap(TRAP_GESTALT);
    c.d[0] = ostype("fs  ");
    CALL(x, &c, &d0, &a0);
    if ((int16_t)d0 != NO_ERR || !(a0 & (1u << 2)))
        return -1;
    c = reg_trap(TRAP_GESTALT);
    c.d[0] = ostype("fsm ");
    CALL(x, &c, &d0, &a0);
    if ((int16_t)d0 != NO_ERR || a0 < 0x0120)
        return -2;

    c = reg_trap(TRAP_NEWPTR_SYS_CLEAR);
    c.d[0] = STACK_SIZE;
    CALL(x, &c, &d0, &a0);
    if (a0 == 0)
        return -3;

    uint32_t stack = a0;

    c = reg_trap(TRAP_NEWPTR_SYS_CLEAR);
    c.d[0] = DATA_SIZE;
    CALL(x, &c, &d0, &a0);
    if (a0 == 0)
        return -3;

    uint32_t data = a0;
    uint32_t fsd = data + DATA_FSD;

    fs->data = data;
    if (fs->volume[0].drive == 0)
    {
        /* Chosen once (the replays see the drives already added). */
        int16_t drive = free_drive(x);

        for (uint32_t i = 0; i < fs->volumes; i++)
            fs->volume[i].drive = (int16_t)(drive + i);
    }

    wr16(x, fsd + FSD_LENGTH, FSD_SIZE);
    wr16(x, fsd + FSD_VERSION, 1);
    wr16(x, fsd + FSD_FSID, FSID);
    wr8(x, fsd + FSD_NAME, sizeof FS_NAME - 1);
    wr_bytes(x, fsd + FSD_NAME + 1, (const uint8_t *)FS_NAME, sizeof FS_NAME - 1);
    wr32(x, fsd + FSD_GLOBALS, data);
    wr32(x, fsd + FSD_COMM_PROC, MAC_STUB_BASE + MAC_FSM_STUB + STUB_COMM);
    wr32(x, fsd + FSD_HFSCI + CI_PROC, MAC_STUB_BASE + MAC_FSM_STUB + STUB_HFS);
    wr32(x, fsd + FSD_HFSCI + CI_STACK_TOP, stack + STACK_SIZE);
    wr32(x, fsd + FSD_HFSCI + CI_STACK_SIZE, STACK_SIZE);
    wr32(x, fsd + FSD_HFSCI + CI_ID_SECTOR, 0xFFFFFFFFu);

    for (uint32_t i = 0; i < fs->volumes; i++)
    {
        uint32_t drive = data + DATA_DRIVE + i * 32u, vmi = data + DATA_VMI + i * 8u;

        wr8(x, drive + DS_DISK_IN_PLACE, 8);       /* a fixed disk */
        wr8(x, drive + DS_INSTALLED, 1);
        wr16(x, drive + DS_QTYPE, 1);              /* hard20: the size fields below are valid */
        wr16(x, drive + DS_DRIVE_SIZE, 0xFFFF);
        wr16(x, drive + DS_DRIVE_S1, 0);
        wr16(x, drive + DS_QFSID, FSID);
        wr16(x, vmi, 8);
        wr32(x, vmi + 2, MEDIA_TYPE);
        wr16(x, vmi + 6, i);                       /* which volume (do_volume_mount) */

        /* AddDrive: D0 = drive number << 16 | driver reference number, A0 = the queue element */
        c = reg_trap(TRAP_ADD_DRIVE);
        c.d[0] = ((uint32_t)(uint16_t)fs->volume[i].drive << 16) | (uint16_t)DISK_REFNUM;
        c.a[0] = drive + DS_QLINK;
        CALL(x, &c, &d0, NULL);
    }

    /* FSMDispatch 0 = InstallFS(fsd) */
    c = reg_trap(TRAP_FSM_DISPATCH);
    c.a[0] = fsd;
    CALL(x, &c, &d0, NULL);
    if ((int16_t)d0 != NO_ERR)
        return (int16_t)d0;

    /* Enable the HFS component: FSMDispatch 5 = SetFSInfo(fsid, size, fsd) */
    wr32(x, fsd + FSD_HFSCI + CI_MASK, rd32(x, fsd + FSD_HFSCI + CI_MASK) | CI_ENABLE | CI_RESOURCE_LOADED | CI_DOES_HFS);
    c = reg_trap(TRAP_FSM_DISPATCH);
    c.d[0] = 5;
    c.d[3] = FSD_SIZE;
    c.d[4] = FSID;
    c.a[0] = fsd;
    CALL(x, &c, &d0, NULL);
    if ((int16_t)d0 != NO_ERR)
        return (int16_t)d0;

    /* HFSDispatch $41 = PBVolumeMount, once per volume: the FSM asks us (comm) and our VolumeMount mounts it. */
    int result = NO_ERR;

    for (uint32_t i = 0; i < fs->volumes; i++)
    {
        wr32(x, data + DATA_PB + IO_BUFFER, data + DATA_VMI + i * 8u);
        c = reg_trap(TRAP_HFS_DISPATCH);
        c.d[0] = 0x41;
        c.a[0] = data + DATA_PB;
        CALL(x, &c, &d0, NULL);
        fs->volume[i].mount_error = (int16_t)d0;
        if (result == NO_ERR)
            result = (int16_t)d0;
    }
    return result;
}

/* ---- The emulated opcodes ---- */

static int comm(mac_t *mac)
{
    mac_fsm_t *fs = &mac->fsm;
    ctx_t x = {mac, fs, NULL, 0, NULL};
    uint32_t sp = m68k_vm_get_reg(M68K_VM_REG_A0 + 7);
    uint32_t message = mac_bus_read(mac, sp + 12, 2);
    uint32_t pb = mac_bus_read(mac, sp + 8, 4);

    switch (message)
    {
    case FFS_NOP:
    case FFS_LOAD:
    case FFS_UNLOAD:
        return NO_ERR;
    case FFS_GET_ICON:
        return AFP_ITEM_NOT_FOUND;      /* the standard disk icon */
    case FFS_ID_DISK:
        for (uint32_t i = 0; i < fs->volumes; i++)
            if ((int16_t)rd16(&x, pb + IO_VREFNUM) == fs->volume[i].drive)
                return NO_ERR;
        return EXT_FS_ERR;
    case FFS_ID_VOL_MOUNT:
        return rd32(&x, rd32(&x, pb + IO_BUFFER) + 2) == MEDIA_TYPE ? NO_ERR : EXT_FS_ERR;
    default:
        return FSM_UNKNOWN_MESSAGE;
    }
}

static void begin(mac_t *mac, uint8_t kind)
{
    mac_fsm_t *fs = &mac->fsm;

    if (fs->depth == MAC_FSM_FRAMES)
    {
        fs->overflow = true;
        return;
    }

    mac_fsm_frame_t *f = &fs->frame[fs->depth++];

    k_memset(f, 0, sizeof *f);
    f->kind = kind;
    if (kind == KIND_HFS)
    {
        /* hfs: in fsm.s saved 56 bytes; then the return address and fsid.w, globals.l, pb.l, select.w, vcb.l */
        uint32_t sp = m68k_vm_get_reg(M68K_VM_REG_A0 + 7) + 56;

        f->globals = mac_bus_read(mac, sp + 6, 4);
        f->pb = mac_bus_read(mac, sp + 10, 4);
        f->select = (uint16_t)mac_bus_read(mac, sp + 14, 2);
        f->vcb = mac_bus_read(mac, sp + 16, 4);
        fs->requests++;
    }
}

static void step(mac_t *mac)
{
    mac_fsm_t *fs = &mac->fsm;
    int result;

    if (fs->overflow || fs->depth == 0 || fs->volumes == 0)
    {
        fs->overflow = false;
        result = PARAM_ERR;
    }
    else
    {
        mac_fsm_frame_t *f = &fs->frame[fs->depth - 1];
        ctx_t x = {mac, fs, f, fs->depth - 1, NULL};

        /* The request's volume: the one of its VCB (none yet for VolumeMount and MountVol). */
        for (uint32_t i = 0; i < fs->volumes; i++)
            if (fs->volume[i].mounted && fs->volume[i].vcb == f->vcb)
                x.v = &fs->volume[i];

        f->next = 0;
        result = f->kind == KIND_INSTALL ? install(&x) : hfs_request(&x);
        if (result == PENDING)
            return;         /* call() has set up the trap call */
        if (f->kind == KIND_INSTALL)
        {
            fs->install_error = (int16_t)result;
        }
        else
        {
            uint32_t i = (fs->requests - 1) % 64;

            fs->log[i].select = f->select;
            fs->log[i].result = (int16_t)result;
            fs->log[i].permission = (uint8_t)mac_bus_read(mac, f->pb + IO_PERMSSN, 1);
        }
        fs->depth--;
    }
    m68k_vm_set_reg(M68K_VM_REG_D0, (uint32_t)(int32_t)(int16_t)result);
    m68k_vm_set_reg(M68K_VM_REG_D0 + 5, 0xFFFFFFFFu);
}

static void result(mac_t *mac)
{
    mac_fsm_t *fs = &mac->fsm;

    if (fs->depth == 0)
        return;

    mac_fsm_frame_t *f = &fs->frame[fs->depth - 1];

    if (f->count < MAC_FSM_CALLS)
    {
        f->d0[f->count] = m68k_vm_get_reg(M68K_VM_REG_D0);
        f->a0[f->count] = m68k_vm_get_reg(M68K_VM_REG_A0);
        f->count++;
    }
    if (fs->restore_addr != 0)
    {
        mac_bus_write(mac, fs->restore_addr, 4, fs->restore_value);
        fs->restore_addr = 0;
    }
}

void mac_fsm_op(mac_t *mac, uint16_t op)
{
    switch (op)
    {
    case MAC_EMUL_OP_FSM_INSTALL:
        begin(mac, KIND_INSTALL);
        break;
    case MAC_EMUL_OP_FSM_HFS:
        begin(mac, KIND_HFS);
        break;
    case MAC_EMUL_OP_FSM_COMM:
        m68k_vm_set_reg(M68K_VM_REG_D0, (uint32_t)(int32_t)comm(mac));
        break;
    case MAC_EMUL_OP_FSM_STEP:
        step(mac);
        break;
    case MAC_EMUL_OP_FSM_RESULT:
        result(mac);
        break;
    default:
        break;
    }
}

int mac_fsm_attach(mac_t *mac, const mac_fs_ops_t *ops)
{
    mac_fsm_t *fs = &mac->fsm;

    if (ops == NULL || fs->volumes == MAC_FSM_VOLUMES)
        return -1;
    fs->volume[fs->volumes++].ops = ops;
    return 0;
}

void mac_fsm_reset(mac_t *mac)
{
    const mac_fs_ops_t *ops[MAC_FSM_VOLUMES];
    uint32_t volumes = mac->fsm.volumes;

    for (uint32_t i = 0; i < volumes; i++)
        ops[i] = mac->fsm.volume[i].ops;
    k_memset(&mac->fsm, 0, sizeof mac->fsm);
    for (uint32_t i = 0; i < volumes; i++)
        mac->fsm.volume[i].ops = ops[i];
    mac->fsm.volumes = volumes;
    mac->fsm.install_pending = volumes > 0;
    k_memcpy(mac->stub + MAC_FSM_STUB, mac_fsm_code, sizeof mac_fsm_code);
}

uint32_t mac_fsm_install_code(void)
{
    return MAC_FSM_STUB + STUB_INSTALL;
}

uint32_t mac_fsm_install_return(void)
{
    return MAC_FSM_STUB + STUB_INSTALL_RETURN;
}
