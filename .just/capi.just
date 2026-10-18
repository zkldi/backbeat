test-capi:
	#!/usr/bin/env bash
	set -euo pipefail

	repo_root="$(pwd)"
	example_dir="$repo_root/rust/backbeat_c_sdk/c_examples"
	include_dir="$repo_root/target/capi/include"
	output_dir="$repo_root/target/capi-tests"
	staticlib="$repo_root/target/debug/libbackbeat_c_sdk.a"
	cc="${CC:-cc}"
	cxx="${CXX:-c++}"

	just gen-cbindings
	mkdir -p "$output_dir"

	case "$(uname -s)" in
		Darwin)
			native_libs=(-lsqlite3 -framework Security -framework SystemConfiguration -framework CoreFoundation -liconv)
			;;
		Linux)
			native_libs=(-lsqlite3 -lgcc_s -lutil -lrt -lpthread -lm -ldl -lc)
			;;
		*)
			echo "unsupported C API test platform: $(uname -s)" >&2
			exit 1
			;;
	esac

	for source in store_info.c asset_read.c version.c; do
		name="${source%.c}"
		"$cc" -I"$include_dir" -std=c11 -Wall -Wextra -Werror \
			"$example_dir/$source" "$staticlib" "${native_libs[@]}" \
			-o "$output_dir/$name"
	done

	"$cxx" -I"$include_dir" -std=c++17 -Wall -Wextra -Werror \
		"$example_dir/cpp_link.cpp" "$staticlib" "${native_libs[@]}" \
		-o "$output_dir/cpp_link"

	"$output_dir/version"
	"$output_dir/cpp_link"
