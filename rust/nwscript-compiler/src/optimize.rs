use crate::opcode::Opcode;

pub const OPTIMIZE_DEAD_FUNCTIONS: u32 = 0x01;
pub const OPTIMIZE_MELD_INSTRUCTIONS: u32 = 0x02;
pub const OPTIMIZE_DEAD_BRANCHES: u32 = 0x04;

pub const OPTIMIZE_NOTHING: u32 = 0x00;
pub const OPTIMIZE_SAFE: u32 = OPTIMIZE_DEAD_FUNCTIONS;
pub const OPTIMIZE_AGGRESSIVE: u32 = OPTIMIZE_SAFE | OPTIMIZE_DEAD_BRANCHES;
pub const OPTIMIZE_EXPERIMENTAL: u32 = OPTIMIZE_AGGRESSIVE | OPTIMIZE_MELD_INSTRUCTIONS;

pub fn optimize_ncs(code: &mut Vec<u8>, flags: u32) {
    if flags == 0 { return; }

    if flags & OPTIMIZE_DEAD_BRANCHES != 0 {
        remove_dead_branches(code);
    }

    if flags & OPTIMIZE_MELD_INSTRUCTIONS != 0 {
        meld_instructions(code);
    }

    // Patch file size after optimization
    if code.len() >= 13 {
        let size = code.len() as i32;
        let b = size.to_be_bytes();
        code[9] = b[0]; code[10] = b[1]; code[11] = b[2]; code[12] = b[3];
    }
}

fn remove_dead_branches(code: &mut Vec<u8>) {
    // Look for patterns:
    //   CONST INT 1 (04 03 00000001)  JZ offset  -> remove both, branch is always taken
    //   CONST INT 0 (04 03 00000000)  JZ offset  -> replace with JMP (branch never taken)
    let mut i = 0;
    while i + 12 <= code.len() {
        // CONST INT = 04 03 XX XX XX XX (6 bytes)
        // JZ        = 1F 00 XX XX XX XX (6 bytes)
        if code[i] == Opcode::Constant as u8 && code[i+1] == 0x03 && i + 12 <= code.len() {
            let val = i32::from_be_bytes([code[i+2], code[i+3], code[i+4], code[i+5]]);
            if code[i+6] == Opcode::Jz as u8 && code[i+7] == 0 {
                if val != 0 {
                    // if(1) — condition always true, remove CONST + JZ, replace with NOPs
                    for j in i..i+12 { code[j] = Opcode::NoOperation as u8; }
                } else {
                    // if(0) — condition always false, change JZ to JMP
                    // Remove the CONST, change JZ to JMP
                    for j in i..i+6 { code[j] = Opcode::NoOperation as u8; }
                    code[i+6] = Opcode::Jmp as u8;
                }
            }
        }
        i += 1;
    }
}

fn meld_instructions(code: &mut Vec<u8>) {
    // Look for adjacent MODIFY_STACK_POINTER instructions and merge them
    // MODIFY_SP = 1B 00 XX XX XX XX (6 bytes)
    let mut i = 0;
    while i + 12 <= code.len() {
        if code[i] == Opcode::ModifyStackPointer as u8 && code[i+1] == 0
            && code[i+6] == Opcode::ModifyStackPointer as u8 && code[i+7] == 0
        {
            let v1 = i32::from_be_bytes([code[i+2], code[i+3], code[i+4], code[i+5]]);
            let v2 = i32::from_be_bytes([code[i+8], code[i+9], code[i+10], code[i+11]]);
            let combined = v1 + v2;

            if combined == 0 {
                // They cancel out — NOP both
                for j in i..i+12 { code[j] = Opcode::NoOperation as u8; }
            } else {
                // Merge into first, NOP the second
                let b = combined.to_be_bytes();
                code[i+2] = b[0]; code[i+3] = b[1]; code[i+4] = b[2]; code[i+5] = b[3];
                for j in i+6..i+12 { code[j] = Opcode::NoOperation as u8; }
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ncs_with_const_jz(const_val: i32, jz_offset: i32) -> Vec<u8> {
        let mut code = vec![0u8; 13]; // header placeholder
        code[0..8].copy_from_slice(b"NCS V1.0");
        code[8] = b'B';

        // CONST INT
        code.push(Opcode::Constant as u8);
        code.push(0x03);
        code.extend_from_slice(&const_val.to_be_bytes());

        // JZ
        code.push(Opcode::Jz as u8);
        code.push(0);
        code.extend_from_slice(&jz_offset.to_be_bytes());

        // RET
        code.push(Opcode::Ret as u8);
        code.push(0);

        let size = code.len() as i32;
        let b = size.to_be_bytes();
        code[9] = b[0]; code[10] = b[1]; code[11] = b[2]; code[12] = b[3];
        code
    }

    #[test]
    fn test_dead_branch_const_true() {
        let mut code = make_ncs_with_const_jz(1, 10);
        let original_len = code.len();
        optimize_ncs(&mut code, OPTIMIZE_DEAD_BRANCHES);
        assert_eq!(code.len(), original_len); // NOPs don't change length
        // Both CONST and JZ should be replaced with NOPs
        assert_eq!(code[13], Opcode::NoOperation as u8);
    }

    #[test]
    fn test_dead_branch_const_false() {
        let mut code = make_ncs_with_const_jz(0, 10);
        optimize_ncs(&mut code, OPTIMIZE_DEAD_BRANCHES);
        // CONST should be NOPed, JZ should become JMP
        assert_eq!(code[13], Opcode::NoOperation as u8);
        assert_eq!(code[19], Opcode::Jmp as u8);
    }

    #[test]
    fn test_meld_modify_sp() {
        let mut code = vec![0u8; 13];
        code[0..8].copy_from_slice(b"NCS V1.0");
        code[8] = b'B';

        // MODIFY_SP -8
        code.push(Opcode::ModifyStackPointer as u8);
        code.push(0);
        code.extend_from_slice(&(-8i32).to_be_bytes());

        // MODIFY_SP -4
        code.push(Opcode::ModifyStackPointer as u8);
        code.push(0);
        code.extend_from_slice(&(-4i32).to_be_bytes());

        let size = code.len() as i32;
        let b = size.to_be_bytes();
        code[9] = b[0]; code[10] = b[1]; code[11] = b[2]; code[12] = b[3];

        optimize_ncs(&mut code, OPTIMIZE_MELD_INSTRUCTIONS);

        // First should now be -12, second should be NOPs
        let merged = i32::from_be_bytes([code[15], code[16], code[17], code[18]]);
        assert_eq!(merged, -12);
        assert_eq!(code[19], Opcode::NoOperation as u8);
    }

    #[test]
    fn test_meld_cancel_out() {
        let mut code = vec![0u8; 13];
        code[0..8].copy_from_slice(b"NCS V1.0");
        code[8] = b'B';

        // MODIFY_SP +4
        code.push(Opcode::ModifyStackPointer as u8);
        code.push(0);
        code.extend_from_slice(&(4i32).to_be_bytes());

        // MODIFY_SP -4
        code.push(Opcode::ModifyStackPointer as u8);
        code.push(0);
        code.extend_from_slice(&(-4i32).to_be_bytes());

        let size = code.len() as i32;
        let b = size.to_be_bytes();
        code[9] = b[0]; code[10] = b[1]; code[11] = b[2]; code[12] = b[3];

        optimize_ncs(&mut code, OPTIMIZE_MELD_INSTRUCTIONS);

        // Both should be NOPs
        assert_eq!(code[13], Opcode::NoOperation as u8);
        assert_eq!(code[19], Opcode::NoOperation as u8);
    }

    #[test]
    fn test_no_optimize_with_zero_flags() {
        let mut code = make_ncs_with_const_jz(1, 10);
        let before = code.clone();
        optimize_ncs(&mut code, OPTIMIZE_NOTHING);
        assert_eq!(code, before);
    }
}
