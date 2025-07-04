import React from "react";
import { Link } from "react-router";
import { FaPlay } from "react-icons/fa";

export function HomePage() {
    return (
        <div className="homepage">
            <main className="content">
                <WorkSection />
            </main>
            <Footer />
        </div>
    );
}

function WorkSection() {
    return (
        <section className="content-section work" id="work">
            {renderHighlight("Cymatics", "/assets/images/cymatics5_cropped.jpg")}
            {renderHighlight("Line", "/assets/images/gravity4_cropped.jpg")}
            {renderHighlight("Flame", "/assets/images/flame.jpg")}
            {renderHighlight("Dots", "/assets/images/dots2.jpg")}
            {renderHighlight("Waves", "/assets/images/waves2.jpg")}
            {renderHighlight("Mito", "/assets/images/mito_cover.png", 'https://hellochar.github.io/mito/#/')}
        </section>
    );
}

function Footer() {
    return (
        <footer className="page-footer">
            <div className="copyright">
                &copy; 2013 - present Xiaohan Zhang
            </div>
        </footer>
    );
}

function renderHighlight(name: string, imageUrl: string, linkUrl?: string) {
    const hasCustomURL = linkUrl != null;
    let innerEl: React.JSX.Element;
    if (hasCustomURL) {
        innerEl = (
            <>
                <figcaption>
                    <a className="work-highlight-name" href={linkUrl} target="_blank">{name}</a>
                </figcaption>
                <a href={linkUrl} target="_blank">
                    <div className="work-highlight-image">
                        <img className="full-width" src={imageUrl} />
                        <div className="work-highlight-sheen sheen-on-hover">
                            <FaPlay />
                        </div>
                    </div>
                </a>
            </>
        );
    } else {
        linkUrl = `/${name.toLowerCase()}`;
        innerEl = (
            <>
                <figcaption>
                    <Link className="work-highlight-name" to={linkUrl}>{name}</Link>
                </figcaption>
                <Link to={linkUrl}>
                    <div className="work-highlight-image">
                        <img className="full-width" src={imageUrl} />
                        <div className="work-highlight-sheen sheen-on-hover">
                            <FaPlay />
                        </div>
                    </div>
                </Link>
            </>
        );
    }
    return (
        <figure className="work-highlight">
            {innerEl}
        </figure>
    );
}
