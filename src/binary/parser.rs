// Copyright 2016 Google Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use mr;
use grammar;
use spirv;

use super::decoder;

use std::result;

use grammar::InstructionTable as GInstTable;
use grammar::OperandKind as GOpKind;
use grammar::OperandQuantifier as GOpCount;

type GInstRef = &'static grammar::Instruction<'static>;

#[derive(Clone, Copy, Debug)]
pub enum State {
    Complete,
    HeaderIncomplete,
    HeaderIncorrect,
    InstructionIncomplete,
    OpcodeUnknown,
    OperandExpected,
}

pub type Result<T> = result::Result<T, State>;

const HEADER_NUM_WORDS: usize = 5;
const MAGIC_NUMBER: spirv::Word = 0x07230203;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParseAction {
    Continue,
    Stop,
}

pub trait Consumer {
    fn consume_header(&mut self, module: mr::ModuleHeader) -> ParseAction;
    fn consume_instruction(&mut self, inst: mr::Instruction) -> ParseAction;
}

pub struct Parser<'a> {
    consumer: &'a mut Consumer,
}

impl<'a> Parser<'a> {
    pub fn new(consumer: &'a mut Consumer) -> Parser<'a> {
        Parser { consumer: consumer }
    }

    pub fn read(self, binary: Vec<u8>) -> Result<()> {
        let mut decoder = decoder::Decoder::new(binary);
        let header = try!(Parser::read_header(&mut decoder));
        if self.consumer.consume_header(header) == ParseAction::Stop {
            return Ok(());
        }

        loop {
            let result = Parser::read_inst(&mut decoder);
            match result {
                Ok(inst) => {
                    if self.consumer.consume_instruction(inst) == ParseAction::Stop {
                        return Ok(());
                    }
                }
                Err(State::Complete) => break,
                Err(error) => return Err(error),
            };
        }
        Ok(())
    }

    fn split_into_word_count_and_opcode(word: spirv::Word) -> (u16, u16) {
        ((word >> 16) as u16, (word & 0xffff) as u16)
    }

    fn read_header(decoder: &mut decoder::Decoder) -> Result<mr::ModuleHeader> {
        if let Ok(words) = decoder.words(HEADER_NUM_WORDS) {
            if words[0] != MAGIC_NUMBER {
                return Err(State::HeaderIncorrect);
            }
            Ok(mr::ModuleHeader::new(words[0], words[1], words[2], words[3], words[4]))
        } else {
            Err(State::HeaderIncomplete)
        }
    }

    fn read_inst(decoder: &mut decoder::Decoder) -> Result<mr::Instruction> {
        if let Ok(word) = decoder.word() {
            let (wc, opcode) = Parser::split_into_word_count_and_opcode(word);
            assert!(wc > 0);
            if let Some(grammar) = GInstTable::lookup_opcode(opcode) {
                decoder.set_limit((wc - 1) as usize);
                let result = Parser::decode_words_to_operands(decoder, grammar);
                assert!(decoder.limit_reached());
                decoder.clear_limit();
                result
            } else {
                Err(State::OpcodeUnknown)
            }
        } else {
            Err(State::Complete)
        }
    }

    fn decode_words_to_operands(decoder: &mut decoder::Decoder,
                                grammar: GInstRef)
                                -> Result<mr::Instruction> {
        let mut rtype = None;
        let mut rid = None;
        let mut concrete_operands = Vec::new();

        let mut logical_operand_index: usize = 0;
        while logical_operand_index < grammar.operands.len() {
            let logical_operand = &grammar.operands[logical_operand_index];
            let has_more_operands = !decoder.limit_reached();
            if has_more_operands {
                match logical_operand.kind {
                    GOpKind::IdResultType => rtype = decoder.id().ok(),
                    GOpKind::IdResult => rid = decoder.id().ok(),
                    _ => {
                        concrete_operands.push(Parser::decode_operand(decoder,
                                                                      logical_operand.kind)
                                                   .unwrap());
                        if let mr::Operand::Decoration(decoration) = *concrete_operands.last()
                                                                                       .unwrap() {
                            concrete_operands.append(
                                &mut Parser::decode_decoration_arguments(
                                    decoder, decoration).unwrap());
                        }
                    }
                }
                match logical_operand.quantifier {
                    GOpCount::One | GOpCount::ZeroOrOne => logical_operand_index += 1,
                    GOpCount::ZeroOrMore => continue,
                }
            } else {
                // We still have logical operands to match but no no more words.
                match logical_operand.quantifier {
                    GOpCount::One => return Err(State::OperandExpected),
                    GOpCount::ZeroOrOne | GOpCount::ZeroOrMore => break,
                }
            }
        }
        Ok(mr::Instruction::new(grammar, rtype, rid, concrete_operands))
    }

    fn decode_operand(decoder: &mut decoder::Decoder, kind: GOpKind) -> Result<mr::Operand> {
        Ok(match kind {
            GOpKind::IdResultType => mr::Operand::IdResultType(decoder.id().unwrap()),
            GOpKind::IdResult => mr::Operand::IdResult(decoder.id().unwrap()),
            GOpKind::IdRef |
            GOpKind::IdMemorySemantics |
            GOpKind::IdScope => mr::Operand::IdRef(decoder.id().unwrap()),
            GOpKind::Scope => mr::Operand::Scope(decoder.scope().unwrap()),
            GOpKind::MemorySemantics => {
                mr::Operand::MemorySemantics(decoder.memory_semantics().unwrap())
            }
            GOpKind::LiteralString => mr::Operand::LiteralString(decoder.string().unwrap()),
            GOpKind::LiteralContextDependentNumber => {
                mr::Operand::LiteralContextDependentNumber(decoder.context_dependent_number()
                                                                  .unwrap())
            }
            GOpKind::Capability => mr::Operand::Capability(decoder.capability().unwrap()),
            GOpKind::Decoration => mr::Operand::Decoration(decoder.decoration().unwrap()),
            GOpKind::AddressingModel => {
                mr::Operand::AddressingModel(decoder.addressing_model()
                                                    .unwrap())
            }
            GOpKind::MemoryModel => mr::Operand::MemoryModel(decoder.memory_model().unwrap()),
            GOpKind::ExecutionMode => {
                mr::Operand::ExecutionMode(decoder.execution_mode()
                                                  .unwrap())
            }
            GOpKind::ExecutionModel => {
                mr::Operand::ExecutionModel(decoder.execution_model().unwrap())
            }
            GOpKind::SourceLanguage => {
                mr::Operand::SourceLanguage(decoder.source_language()
                                                   .unwrap())
            }
            GOpKind::LiteralInteger => mr::Operand::LiteralInteger(decoder.integer().unwrap()),
            GOpKind::StorageClass => mr::Operand::StorageClass(decoder.storage_class().unwrap()),
            GOpKind::ImageOperands => mr::Operand::ImageOperands(decoder.image_operands().unwrap()),
            GOpKind::FPFastMathMode => {
                mr::Operand::FPFastMathMode(decoder.fpfast_math_mode().unwrap())
            }
            GOpKind::SelectionControl => {
                mr::Operand::SelectionControl(decoder.selection_control().unwrap())
            }
            GOpKind::LoopControl => mr::Operand::LoopControl(decoder.loop_control().unwrap()),
            GOpKind::FunctionControl => {
                mr::Operand::FunctionControl(decoder.function_control().unwrap())
            }
            GOpKind::MemoryAccess => mr::Operand::MemoryAccess(decoder.memory_access().unwrap()),
            GOpKind::KernelProfilingInfo => {
                mr::Operand::KernelProfilingInfo(decoder.kernel_profiling_info()
                                                        .unwrap())
            }
            GOpKind::Dim => mr::Operand::Dim(decoder.dim().unwrap()),
            GOpKind::SamplerAddressingMode => {
                mr::Operand::SamplerAddressingMode(decoder.sampler_addressing_mode()
                                                          .unwrap())
            }
            GOpKind::SamplerFilterMode => {
                mr::Operand::SamplerFilterMode(decoder.sampler_filter_mode().unwrap())
            }
            GOpKind::ImageFormat => mr::Operand::ImageFormat(decoder.image_format().unwrap()),
            GOpKind::ImageChannelOrder => {
                mr::Operand::ImageChannelOrder(decoder.image_channel_order().unwrap())
            }
            GOpKind::ImageChannelDataType => {
                mr::Operand::ImageChannelDataType(decoder.image_channel_data_type()
                                                         .unwrap())
            }
            GOpKind::FPRoundingMode => {
                mr::Operand::FPRoundingMode(decoder.fprounding_mode().unwrap())
            }
            GOpKind::LinkageType => mr::Operand::LinkageType(decoder.linkage_type().unwrap()),
            GOpKind::AccessQualifier => {
                mr::Operand::AccessQualifier(decoder.access_qualifier().unwrap())
            }
            GOpKind::FunctionParameterAttribute => {
                mr::Operand::FunctionParameterAttribute(decoder.function_parameter_attribute()
                                                               .unwrap())
            }
            GOpKind::BuiltIn => mr::Operand::BuiltIn(decoder.built_in().unwrap()),
            GOpKind::GroupOperation => {
                mr::Operand::GroupOperation(decoder.group_operation().unwrap())
            }
            GOpKind::KernelEnqueueFlags => {
                mr::Operand::KernelEnqueueFlags(decoder.kernel_enqueue_flags().unwrap())
            }
            GOpKind::LiteralExtInstInteger |
            GOpKind::LiteralSpecConstantOpInteger |
            GOpKind::PairLiteralIntegerIdRef |
            GOpKind::PairIdRefLiteralInteger |
            GOpKind::PairIdRefIdRef => {
                println!("unimplemented operand kind: {:?}", kind);
                unimplemented!();
            }
        })
    }

    fn decode_decoration_arguments(decoder: &mut decoder::Decoder,
                                   decoration: spirv::Decoration)
                                   -> Result<Vec<mr::Operand>> {
        match decoration {
            spirv::Decoration::BuiltIn => {
                Ok(vec![mr::Operand::BuiltIn(decoder.built_in().unwrap())])
            }
            spirv::Decoration::Block => Ok(vec![]),
            _ => unimplemented!(),

        }
    }
}

pub fn parse(binary: Vec<u8>, consumer: &mut Consumer) -> Result<()> {
    Parser::new(consumer).read(binary)
}
