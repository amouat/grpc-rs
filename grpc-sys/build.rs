// Copyright 2017 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

extern crate cc;
extern crate cmake;
extern crate pkg_config;

use std::path::Path;
use std::{env, fs, io};
use std::env::VarError;

use cmake::Config;
use cc::Build;
use pkg_config::{Config as PkgConfig, Library};

const GRPC_VERSION: &'static str = "1.13.0";

fn probe_library(library: &str, cargo_metadata: bool) -> Library {
    match PkgConfig::new()
        .atleast_version(GRPC_VERSION)
        .cargo_metadata(cargo_metadata)
        .probe(library)
    {
        Ok(lib) => lib,
        Err(e) => panic!("can't find library {} via pkg-config: {:?}", library, e),
    }
}

fn prepare_grpc() {
    let mut modules = vec![
        "grpc",
        "grpc/third_party/zlib",
        "grpc/third_party/cares/cares",
        "grpc/third_party/address_sorting",
    ];

    if cfg!(feature = "secure") {
        modules.push("grpc/third_party/boringssl");
    }

    for module in modules {
        if is_directory_empty(module).unwrap_or(true) {
            panic!(
                "Can't find module {}. You need to run `git submodule \
                 update --init --recursive` first to build the project.",
                module
            );
        }
    }
}

fn is_directory_empty<P: AsRef<Path>>(p: P) -> Result<bool, io::Error> {
    let mut entries = fs::read_dir(p)?;
    Ok(entries.next().is_none())
}

fn build_grpc(cc: &mut Build, library: &str, library_cpp: &str) {
    prepare_grpc();

    let dst = {
        let mut config = Config::new("grpc");
        if !cfg!(feature = "secure") {
            // boringssl's configuration is still included, but targets
            // will never be built, hence specify a fake go to get rid of
            // the unnecessary dependency.
            config.define("GO_EXECUTABLE", "fake-go-nonexist");
        }
        if cfg!(target_os = "macos") {
            config.cxxflag("-stdlib=libc++");
            // As cmake CMP0042 suggests.
            config.define("CMAKE_MACOSX_RPATH", "ON");
        }
        if env::var("CARGO_CFG_TARGET_ENV").unwrap_or("".to_owned()) == "musl" {
            config.define("CMAKE_CXX_COMPILER", "g++");
        }
        // We dont need generate install targets.
        config.define("gRPC_INSTALL", "false");
        // Target grpc++ also builds grpc.
        config.build_target(library_cpp).uses_cxx11().build()
    };

    let mut zlib = "z";
    let build_dir = format!("{}/build", dst.display());
    let third_party = vec![
        "cares/cares/lib",
        "zlib",
        "boringssl/ssl",
        "boringssl/crypto",
    ];
    if cfg!(target_os = "windows") {
        let profile = match &*env::var("PROFILE").unwrap_or("debug".to_owned()) {
            "bench" | "release" => {
                zlib = "zlibstatic";
                "Release"
            }
            _ => {
                zlib = "zlibstaticd";
                "Debug"
            }
        };
        println!("cargo:rustc-link-search=native={}/{}", build_dir, profile);
        for path in third_party {
            println!(
                "cargo:rustc-link-search=native={}/third_party/{}/{}",
                build_dir, path, profile
            );
        }
    } else {
        println!("cargo:rustc-link-search=native={}", build_dir);
        for path in third_party {
            println!(
                "cargo:rustc-link-search=native={}/third_party/{}",
                build_dir, path,
            );
        }
    }

    println!("cargo:rustc-link-lib=static={}", zlib);
    println!("cargo:rustc-link-lib=static=cares");
    println!("cargo:rustc-link-lib=static=gpr");
    println!("cargo:rustc-link-lib=static=address_sorting");
    println!("cargo:rustc-link-lib=static={}", library);
    println!("cargo:rustc-link-lib=static={}", library_cpp);

    if cfg!(feature = "secure") {
        println!("cargo:rustc-link-lib=static=ssl");
        println!("cargo:rustc-link-lib=static=crypto");
    }

    cc.include("grpc/include");
}

fn get_env(name: &str) -> Option<String> {
    println!("cargo:rerun-if-env-changed={}", name);
    match env::var(name) {
        Ok(s) => Some(s),
        Err(VarError::NotPresent) => None,
        Err(VarError::NotUnicode(s)) => {
            panic!("unrecognize env var of {}: {:?}", name, s.to_string_lossy());
        }
    }
}

fn main() {
    let mut cc = Build::new();

    println!("cargo:rerun-if-changed=grpc_wrap.c");
    println!("cargo:rerun-if-changed=grpc");

    let (library_c, library_cpp) = if cfg!(feature = "secure") {
        cc.define("GRPC_SYS_SECURE", None);
        ("grpc", "grpc++")
    } else {
        ("grpc_unsecure", "grpc++_unsecure")
    };

    let use_pkg_config = get_env("GRPCIO_SYS_USE_PKG_CONFIG").map_or(false, |s| s == "1");

    if use_pkg_config {
        // Do not print cargo metadata.
        let lib_core = probe_library(library_c, false);
        for inc_path in lib_core.include_paths {
            cc.include(inc_path);
        }
        let lib_cpp = probe_library(library_cpp, false);
        for inc_path in lib_cpp.include_paths {
            cc.include(inc_path);
        }
    } else {
        build_grpc(&mut cc, library_c, library_cpp);
    }

    cc.cpp(true);
    if !cfg!(target_env = "msvc") {
        cc.flag("-std=c++11");
    }
    cc.file("grpc_wrap.cc");

    if cfg!(target_os = "windows") {
        // At lease win7
        cc.define("_WIN32_WINNT", Some("0x0700"));
    }

    cc.warnings_into_errors(true);
    cc.compile("libgrpc_wrap.a");

    if use_pkg_config {
        // Link libgrpc.so and libgrpc++.so.
        probe_library(library_c, true);
        probe_library(library_cpp, true);
    }
}
