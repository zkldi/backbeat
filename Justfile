import ".just/cli.just"
import ".just/test.just"
import ".just/capi.just"
import ".just/app.just"
import ".just/app-vf.just"
import ".just/app-packager.just"
import ".just/docker.just"
import ".just/services.just"
import ".just/gen.just"
import ".just/website.just"

[private]
interactive:
	-@just --choose
