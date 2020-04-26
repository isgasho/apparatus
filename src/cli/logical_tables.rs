use log::{debug};

use crate::Result;
use crate::error::Error;
use crate::buf::Reading;

// II.24.2.6: The physical representation of a row cell e at a
// column with type C is defined as follows: 
// - If e is a constant, it is stored using the number of bytes as
// specified for its column type C (i.e., a 2-bit mask of type
// PropertyAttributes).
// - If e is an index into the GUID heap, 'blob', or String heap,
// it is stored using the number of bytes as defined in the
// HeapSizes field.
//- If e is a simple index into a table with index i, it is stored
// using 2 bytes if table i has less than 2^16 rows, otherwise it
// is stored using 4 bytes.
// - If e is a coded index that points into table ti out of n
// possible tables t0, ...tn-1, then it is stored as
// e << (log n) | tag{ t0, ...tn-1}[ti] using 2 bytes if
// the maximum number of rows of tables t0, ...tn-1, is
// less than 2^(16 - (log n)), and using 4 bytes otherwise.
// The family of finite maps tag {t0, ...tn-1} is defined below.
// Note that decoding a physical row requires the inverse of this
// mapping. [For example, the Parent column of the Constant table
// indexes a row in the Field, Param, or Property tables.  The
// actual table is encoded into the low 2 bits of the number,
// using the values: 0 => Field, 1 => Param, 2 => Property.The
// remaining bits hold the actual row number being indexed.  For
// example, a value of 0x321, indexes row number 0xC8 in the Param
// table.]

// Taken from ECMA II.22
const METADATA_MODULE:                   usize = 0x00;
const METADATA_TYPE_REF:                 usize = 0x01;
const METADATA_TYPE_DEF:                 usize = 0x02;
const METADATA_FIELD:                    usize = 0x04;
const METADATA_METHOD_DEF:               usize = 0x06;
const METADATA_PARAM:                    usize = 0x08;
const METADATA_INTERFACE_IMPL:           usize = 0x09;
const METADATA_MEMBER_REF:               usize = 0x0A;
const METADATA_CONSTANT:                 usize = 0x0B;
const METADATA_CUSTOM_ATTRIBUTE:         usize = 0x0C;
const METADATA_FIELD_MARSHAL:            usize = 0x0D;
const METADATA_DECL_SECURITY:            usize = 0x0E;
const METADATA_CLASS_LAYOUT:             usize = 0x0F;
const METADATA_FIELD_LAYOUT:             usize = 0x10;
const METADATA_STANDALONE_SIG:           usize = 0x11;
const METADATA_EVENT_MAP:                usize = 0x12;
const METADATA_EVENT:                    usize = 0x14;
const METADATA_PROPERTY_MAP:             usize = 0x15;
const METADATA_PROPERTY:                 usize = 0x17;
const METADATA_METHOD_SEMANTICS:         usize = 0x18;
const METADATA_METHOD_IMPL:              usize = 0x19;
const METADATA_MODULE_REF:               usize = 0x1A;
const METADATA_TYPE_SPEC:                usize = 0x1B;
const METADATA_IMPL_MAP:                 usize = 0x1C;
const METADATA_FIELD_RVA:                usize = 0x1D;
const METADATA_ASSEMBLY:                 usize = 0x20;
const METADATA_ASSEMBLY_PROCESSOR:       usize = 0x21;
const METADATA_ASSEMBLY_OS:              usize = 0x22;
const METADATA_ASSEMBLY_REF:             usize = 0x23;
const METADATA_ASSEMBLY_REFPROCESSOR:    usize = 0x24;
const METADATA_ASSEMBLY_REFOS:           usize = 0x25;
const METADATA_FILE:                     usize = 0x26;
const METADATA_EXPORTED_TYPE:            usize = 0x27;
const METADATA_MANIFEST_RESOURCE:        usize = 0x28;
const METADATA_NESTED_CLASS:             usize = 0x29;
const METADATA_GENERIC_PARAM:            usize = 0x2A;
const METADATA_METHOD_SPEC:              usize = 0x2B;
const METADATA_GENERIC_PARAM_CONSTRAINT: usize = 0x2C;

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum IndexSize {
	/// 2-byte index.
	U16,
	/// 4-byte index.
	U32,
}

#[derive(Copy, Clone)]
pub struct Tables {
	pub string_index_size: IndexSize,
	pub guid_index_size:   IndexSize,
	pub blob_index_size:   IndexSize,
	pub lens: [u32; 64],
	pub size: usize,
	valid_mask: u64,
}

impl Tables {
	pub fn parse(data: &[u8]) -> Result<Tables> {
		let mut offset = &mut 0usize;

		// Reserverd1, major version, minor version.
		*offset += 6;
		
		// The HeapSizes field is a bitvector that encodes the width of
		// indexes into the various heaps. If bit 0 is set, indexes into
		// the #String heap are 4 bytes wide; if bit 1 is set, indexes
		// into the #GUID heap are 4 bytes wide; if bit 2 is set, indexes
		// into the #Blob heap are 4 bytes wide. Conversely, if the
		// HeapSize bit for a particular heap is not set, indexes into
		// that heap are 2 bytes wide.
		let heap_sizes: u8 = data.read(offset)?;
		debug!("Heap sizes: {:#010b}", heap_sizes);

		let string_index_size = if heap_sizes & 0x01 == 0 { IndexSize::U16 } else { IndexSize::U32 };
		let guid_index_size   = if heap_sizes & 0x02 == 0 { IndexSize::U16 } else { IndexSize::U32 };
		let blob_index_size   = if heap_sizes & 0x04 == 0 { IndexSize::U16 } else { IndexSize::U32 };

		// Reserved2
		*offset += 1;
		
		// The Valid field is a 64-bit bitvector that has a specific bit
		// set for each table that is stored in the stream; the mapping of
		// tables to indexes is given at the start of II.22.
		let valid_mask: u64 = data.read(offset)?;
		let n = valid_mask.count_ones() as usize;
		debug!("Valid mask: {:#066b} -> {} table(s).", valid_mask, n);

		// Sorted.
		*offset += 8;
		
		let mut lens = [0u32; 64];
		for i in 0..lens.len() {
			if (valid_mask >> i) & 1 == 1 {
				lens[i] = data.read(offset)?;
				debug!("Table #{} has {:#0x} item(s).", i, lens[i]);
			}
		}

		let size = *offset;
		
		Ok(Tables {
			string_index_size,
			guid_index_size,
			blob_index_size,
			lens,
			size,
			valid_mask,
		})
	}

	fn has_table(&self, id: usize) -> bool {
		(self.valid_mask >> id) & 1 == 1
	}
}

#[derive(Debug, PartialEq, Clone, Default)]
pub struct StringIndex(u32);

#[derive(Debug, PartialEq, Clone, Default)]
pub struct GuidIndex(u32);

impl StringIndex {
	fn parse(header: &Tables, data: &[u8], offset: &mut usize) -> Result<Self> {
		let i = match header.string_index_size {
			IndexSize::U16 => StringIndex(data.read::<u16>(offset)? as u32),
			IndexSize::U32 => StringIndex(data.read::<u32>(offset)? as u32),
		};
		Ok(i)
	}
}

impl GuidIndex {
	fn parse(header: &Tables, data: &[u8], offset: &mut usize) -> Result<Self> {
		let i = match header.guid_index_size {
			IndexSize::U16 => GuidIndex(data.read::<u16>(offset)? as u32),
			IndexSize::U32 => GuidIndex(data.read::<u32>(offset)? as u32),
		};
		Ok(i)
	}
}

macro_rules! max {
	($x:expr) => ($x);
	($x:expr, $($xs:expr),+) => {
		{
			use std::cmp::max;
			max($x, max!($($xs),+))
		}
	};
}

macro_rules! count_tts {
    () => {0usize};
    ($_head:tt $($tail:tt)*) => {1usize + count_tts!($($tail)*)};
}

const fn size_for_big_index(n: usize) -> usize { 1 << (16 - log2(n)) }
const fn log2(x: usize) -> usize { 64usize - x.leading_zeros() as usize }

macro_rules! coded_index {
	($name:ident, $bits:expr, $(($v:ident $t:expr, $id:ident))+) => {
		#[derive(Debug, PartialEq, Copy, Clone)]
		pub enum $name {
			$($v(u32),)+
		}

		impl $name {
			fn parse(header: &Tables, data: &[u8], offset: &mut usize) -> Result<$name> {
				const COUNT: usize = count_tts!($($v),+);
				let max_len = max!($(header.lens[$id]),+) as usize;
				let value = if max_len < size_for_big_index(COUNT) {
					data.read::<u16>(offset)? as u32
				} else {
					data.read::<u32>(offset)?
				};

				let tag = value & (1 << $bits) - 1;
				let idx = value >> $bits;
				
				let r = match tag {
					$(
						$t => $name::$v(idx),
					)+
					_ => Err("Unknown coded index tag.")?,
				};

				Ok(r)
			}
		}
	};
}

// II 24.2.6

coded_index!(TypeDefOrRef, 2,
	(TypeDef  0, METADATA_TYPE_DEF)
	(TypeRef  1, METADATA_TYPE_REF)
	(TypeSpec 2, METADATA_TYPE_SPEC));

coded_index!(HasConstant, 2,
	(Field    0, METADATA_FIELD)
	(Param    1, METADATA_PARAM)
	(Property 2, METADATA_PROPERTY));

coded_index!(HasCustomAttribute, 5,
	(MethodDef               0, METADATA_METHOD_DEF)
	(Field                   1, METADATA_FIELD)
	(TypeRef                 2, METADATA_TYPE_REF)
	(TypeDef                 3, METADATA_TYPE_DEF)
	(Param                   4, METADATA_PARAM)
	(InterfaceImpl           5, METADATA_INTERFACE_IMPL)
	(MemberRef               6, METADATA_MEMBER_REF)
	(Module                  7, METADATA_MODULE)
	(Permission              8, METADATA_DECL_SECURITY)
	(Property                9, METADATA_PROPERTY)
	(Event                  10, METADATA_EVENT)
	(StandAloneSig          11, METADATA_STANDALONE_SIG)
	(ModuleRef              12, METADATA_MODULE_REF)
	(TypeSpec               13, METADATA_TYPE_SPEC)
	(Assembly               14, METADATA_ASSEMBLY)
	(AssemblyRef            15, METADATA_ASSEMBLY_REF)
	(File                   16, METADATA_FILE)
	(ExportedType           17, METADATA_EXPORTED_TYPE)
	(ManifestResource       18, METADATA_MANIFEST_RESOURCE)
	(GenericParam           19, METADATA_GENERIC_PARAM)
	(GenericParamConstraint 20, METADATA_GENERIC_PARAM_CONSTRAINT)
	(MethodSpec             21, METADATA_METHOD_SPEC));

coded_index!(HasFieldMarshall, 1,
	(Field 0, METADATA_FIELD)
	(Param 1, METADATA_PARAM));

coded_index!(HasDeclSecurity, 2,
	(TypeDef   0, METADATA_TYPE_DEF)
	(MethodDef 1, METADATA_METHOD_DEF)
	(Assembly  2, METADATA_ASSEMBLY));

coded_index!(MemberRefParent, 3,
	(TypeDef   0, METADATA_TYPE_DEF)
	(TypeRef   1, METADATA_TYPE_REF)
	(ModuleRef 2, METADATA_MODULE_REF)
	(MethodDef 3, METADATA_METHOD_DEF)
	(TypeSpec  4, METADATA_TYPE_SPEC));

coded_index!(HasSemantics, 1,
	(Event    0, METADATA_EVENT)
	(Property 1, METADATA_PROPERTY));

coded_index!(MethodDefOrRef, 1,
	(MethodDef 0, METADATA_METHOD_DEF)
	(MemberRef 1, METADATA_MEMBER_REF));

coded_index!(MemberForwarded, 1,
	(Field     0, METADATA_FIELD)
	(MethodDef 1, METADATA_METHOD_DEF));

coded_index!(Implementation, 2,
	(File         0, METADATA_FILE)
	(AssemblyRef  1, METADATA_ASSEMBLY_REF)
	(ExportedType 2, METADATA_EXPORTED_TYPE));

coded_index!(CustomAttributeType, 3,
	(MethodDef 2, METADATA_METHOD_DEF)
	(MemberRef 3, METADATA_MEMBER_REF));

coded_index!(ResolutionScope, 2,
	(Module      0, METADATA_MODULE)
	(ModuleRef   1, METADATA_MODULE_REF)
	(AssemblyRef 2, METADATA_ASSEMBLY_REF)
	(TypeRef     3, METADATA_TYPE_REF));

#[derive(Debug, PartialEq, Clone, Default)]
pub struct TableRows {
	modules: Box<[Module]>,
	type_refs: Box<[TypeRef]>,
}

impl TableRows {
	pub fn parse(header: &Tables, data: &[u8]) -> Result<Self> {
		let mut offset = &mut 0;

		macro_rules! table {
			($table:ident, $id:ident, $type:ty) => {
				let $table = if header.has_table($id) {
					<$type>::parse_many(header, data, offset)?
				} else {
					empty()
				};
			};
		}

		table!(modules,   METADATA_MODULE,   Module);
		table!(type_refs, METADATA_TYPE_REF, TypeRef);
		
		Ok(TableRows {
			modules,
			type_refs,
		})
	}
}

/// II.22.30
#[derive(Debug, PartialEq, Clone)]
pub struct Module {
	/// Module name.
	pub name: StringIndex,
	/// Simply a Guid used to distinguish between two
	/// versions of the same module.
	pub mvid: GuidIndex,
}

impl Module {
	fn parse_many(header: &Tables, data: &[u8], offset: &mut usize) -> Result<Box<[Module]>> {
		let n = header.lens[METADATA_MODULE] as usize;
		let mut result = Vec::with_capacity(n);

		for i in 0..n {
			let generation: u16 = data.read(offset)?;
			if generation != 0 {
				Err("Module has invalid generation.")?;
			}

			let name = StringIndex::parse(header, data, offset)?;
			let mvid = GuidIndex::parse(header, data, offset)?;

			let enc_id: u16 = data.read(offset)?;
			if enc_id != 0 {
				Err("Module.EncId is not zero.")?;
			}
			let enc_base_id: u16 = data.read(offset)?;
			if enc_base_id != 0 {
				Err("Module.EncBaseId is not zero.")?;
			}

			result.push(Module { name, mvid })
		}

		Ok(result.into_boxed_slice())
	}
}

/// II.24.2.6
#[derive(Debug, PartialEq, Clone)]
pub struct TypeRef {
	pub scope: ResolutionScope,
	pub name: StringIndex,
	pub namespace: StringIndex,
}

impl TypeRef {
	fn parse_many(header: &Tables, data: &[u8], offset: &mut usize) -> Result<Box<[TypeRef]>> {
		let n = header.lens[METADATA_TYPE_REF] as usize;
		let mut result = Vec::with_capacity(n);

		for i in 0..n {
			let scope = ResolutionScope::parse(header, data, offset)?;
			let name = StringIndex::parse(header, data, offset)?;
			let namespace = StringIndex::parse(header, data, offset)?;

			result.push(TypeRef { scope, name, namespace })
		}

		Ok(result.into_boxed_slice())
	}
}

fn empty<T>() -> Box<[T]> {
	Box::new([])
}
