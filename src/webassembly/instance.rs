//! An `Instance` contains all the runtime state used by execution of a wasm
//! module.
use cranelift_codegen::ir;
use cranelift_wasm::GlobalIndex;
use std::mem::transmute;
use std::ptr;
use std::any::Any;
use cranelift_codegen::ir::{AbiParam, types};


use super::memory::LinearMemory;
use super::module::{DataInitializer, Module, Export, TableElements};
use super::compilation::Compilation;
use super::execute::make_vmctx;

/// An Instance of a WebAssemby module.
#[derive(Debug)]
pub struct Instance {
    // pub module: Box<Module>,

    // pub compilation: Box<Compilation>,

    /// WebAssembly table data.
    pub tables: Vec<Vec<usize>>,

    /// WebAssembly linear memory data.
    pub memories: Vec<LinearMemory>,

    /// WebAssembly global variable data.
    pub globals: Vec<u8>,
}

#[derive(Debug)]
pub enum InvokeResult {
    VOID,
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}

impl Instance {
    /// Create a new `Instance`.
    pub fn new(
        module: &Module,
        compilation: &Compilation,
        data_initializers: &[DataInitializer],
    ) -> Self {
        let mut result = Self {
            // module: Box::new(module),
            // compilation: Box::new(compilation),
            tables: Vec::new(),
            memories: Vec::new(),
            globals: Vec::new(),
        };
        // println!("Instance::instantiate tables");
        result.instantiate_tables(module, compilation, &module.table_elements);
        // println!("Instance::instantiate memories");
        result.instantiate_memories(module, data_initializers);
        // println!("Instance::instantiate globals");
        result.instantiate_globals(module);
        result
    }

    /// Allocate memory in `self` for just the tables of the current module.
    fn instantiate_tables(
        &mut self,
        module: &Module,
        compilation: &Compilation,
        table_initializers: &[TableElements],
    ) {
        debug_assert!(self.tables.is_empty());
        self.tables.reserve_exact(module.tables.len());
        for table in &module.tables {
            let len = table.size;
            let mut v = Vec::with_capacity(len);
            v.resize(len, 0);
            self.tables.push(v);
        }
        for init in table_initializers {
            debug_assert!(init.base.is_none(), "globalvar base not supported yet");
            let to_init =
                &mut self.tables[init.table_index][init.offset..init.offset + init.elements.len()];
            for (i, func_idx) in init.elements.iter().enumerate() {
                let code_buf = &compilation.functions[module.defined_func_index(*func_idx).expect(
                    "table element initializer with imported function not supported yet",
                )];
                to_init[i] = code_buf.as_ptr() as usize;
            }
        }
    }

    /// Allocate memory in `instance` for just the memories of the current module.
    fn instantiate_memories(&mut self, module: &Module, data_initializers: &[DataInitializer]) {
        debug_assert!(self.memories.is_empty());
        // Allocate the underlying memory and initialize it to all zeros.
        // println!("instantiate_memories::reserve exact");
        self.memories.reserve_exact(module.memories.len());
        // println!("instantiate_memories::loop");
        for memory in &module.memories {
            // println!("instantiate_memories::new linear memory: {}", memory.pages_count);
            // We do this so at least there is one page
            let pages_count = if (memory.pages_count as u32) > 0 {
                memory.pages_count as u32
            }
            else  {
                1
            };
            let v = LinearMemory::new(pages_count, memory.maximum.map(|m| m as u32));
            self.memories.push(v);
        }
        for init in data_initializers {
            // println!("instantiate_memories::initialize data");
            debug_assert!(init.base.is_none(), "globalvar base not supported yet");
            let mem_mut = self.memories[init.memory_index].as_mut();
            let to_init = &mut mem_mut[init.offset..init.offset + init.data.len()];
            to_init.copy_from_slice(init.data);
        }
    }

    /// Allocate memory in `instance` for just the globals of the current module,
    /// without any initializers applied yet.
    fn instantiate_globals(&mut self, module: &Module) {
        debug_assert!(self.globals.is_empty());
        // Allocate the underlying memory and initialize it to all zeros.
        let globals_data_size = module.globals.len() * 8;
        self.globals.resize(globals_data_size, 0);
    }

    /// Returns a mutable reference to a linear memory under the specified index.
    pub fn memory_mut(&mut self, memory_index: usize) -> &mut LinearMemory {
        self.memories
            .get_mut(memory_index)
            .unwrap_or_else(|| panic!("no memory for index {}", memory_index))
    }

    /// Returns a slice of the contents of allocated linear memory.
    pub fn inspect_memory(&self, memory_index: usize, address: usize, len: usize) -> &[u8] {
        &self
            .memories
            .get(memory_index)
            .unwrap_or_else(|| panic!("no memory for index {}", memory_index))
            .as_ref()[address..address + len]
    }

    /// Shows the value of a global variable.
    pub fn inspect_global(&self, global_index: GlobalIndex, ty: ir::Type) -> &[u8] {
        let offset = global_index * 8;
        let len = ty.bytes() as usize;
        &self.globals[offset..offset + len]
    }


    pub fn execute_fn(
        &mut self,
        module: &Module,
        compilation: &Compilation,
        func_name: String,
    ) -> Result<InvokeResult, String> {
        // println!("execute");
        // println!("TABLES: {:?}", self.tables);
        // println!("MEMORIES: {:?}", self.memories);
        // println!("GLOBALS: {:?}", self.globals);

        let export_func = module.exports.get(&func_name);
        let func_index = match export_func {
            Some(&Export::Function(index)) => index,
            _ => panic!("No func name")
        };

        let code_buf = &compilation.functions[module
                                    .defined_func_index(func_index)
                                    .expect("imported start functions not supported yet")];

        let sig_index = module.functions[func_index];
        let imported_sig = &module.signatures[sig_index];

        // println!("FUNCTION CODE BUF={:?}", imported_sig);

        // Collect all memory base addresses and Vec.
        let mut mem_base_addrs = self
            .memories
            .iter_mut()
            .map(LinearMemory::base_addr)
            .collect::<Vec<_>>();
        let vmctx = make_vmctx(self, &mut mem_base_addrs);

        // unsafe {
        //     func = transmute::<_, fn(*const *mut u8) -> Box<Any>>(code_buf.as_ptr());
        // }
        // ret = ;
        match imported_sig.returns.len() {
            0 => unsafe {
                let func = transmute::<_, fn(*const *mut u8)>(code_buf.as_ptr());
                func(vmctx.as_ptr());
                Ok(InvokeResult::VOID)
            },
            1 => {
                let value_type = imported_sig.returns[0].value_type;
                match value_type {
                    types::I32 => unsafe {
                        let func = transmute::<_, fn(*const *mut u8) -> i32>(code_buf.as_ptr());
                        Ok(InvokeResult::I32(func(vmctx.as_ptr())))
                    },
                    types::I64 => unsafe {
                        let func = transmute::<_, fn(*const *mut u8) -> i64>(code_buf.as_ptr());
                        Ok(InvokeResult::I64(func(vmctx.as_ptr())))
                    },
                    types::F32 => unsafe {
                        let func = transmute::<_, fn(*const *mut u8) -> f32>(code_buf.as_ptr());
                        Ok(InvokeResult::F32(func(vmctx.as_ptr())))
                    },
                    types::F64 => unsafe {
                        let func = transmute::<_, fn(*const *mut u8) -> f64>(code_buf.as_ptr());
                        Ok(InvokeResult::F64(func(vmctx.as_ptr())))
                    },
                    _ => panic!("Invalid signature")
                }
            },
            _ => panic!("Only one-returnf functions are supported for now")
        }

        // println!("TABLES: {:?}", self.tables);
        // println!("MEMORIES: {:?}", self.memories);
        // println!("{:?}", module.exports);
        // println!("execute end");


        
    }

}
